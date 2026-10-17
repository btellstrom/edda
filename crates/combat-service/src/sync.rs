//! Keeps character sheets in step with combats: hit points are written
//! back, and what casting or using a feature costs (a slot, uses) is spent
//! on the sheet before the command runs and given back if it's refused or
//! undone. Every frontend gets this without doing anything.
//!
//! The combat log stays the authority during a fight; the character service
//! follows it. How far each combat has been written back is stored in
//! `combat_hp_sync`, so a write-back that fails (the character service is
//! down, say) is retried later by whichever instance gets there first.
//! Only one instance syncs a given combat at a time, and it always writes
//! the latest HP, so sheets never go backwards. Spending and giving back
//! carry an idempotency key, so a retry never does either twice.
//!
//! It writes as this service itself (see [`crate::services`]).

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use edda_client::{Client, ClientError};
use edda_types::{
    HitPoints,
    combat::{CombatEvent, CombatState, LoggedEvent, SheetCost},
};
use uuid::Uuid;

use crate::{error::ApiError, services::ServiceLogin, store::Store};

/// How often combats whose write-back failed are retried.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

pub struct SheetSync {
    store: Store,
    login: Arc<ServiceLogin>,
    character_service: String,
}

/// Idempotency keys: what a command spends is keyed by the command itself;
/// giving it back has a key of its own for each reason.
fn give_back_key(command: Uuid, why: &str) -> Uuid {
    Uuid::new_v5(&command, why.as_bytes())
}

/// Giving back what a refused command spent.
pub fn refused_key(command: Uuid) -> Uuid {
    give_back_key(command, "refused")
}

fn undone_key(command: Uuid) -> Uuid {
    give_back_key(command, "undone")
}

#[derive(Debug, thiserror::Error)]
enum SyncError {
    #[error(transparent)]
    Store(#[from] ApiError),
    #[error("character service: {0}")]
    Characters(#[from] ClientError),
}

impl SheetSync {
    pub fn new(store: Store, login: Arc<ServiceLogin>, character_service: String) -> Self {
        Self {
            store,
            login,
            character_service,
        }
    }

    /// A character service client authorised for `campaign`.
    async fn characters(&self, campaign: Uuid) -> Result<Client, ClientError> {
        let token = self.login.token(campaign).await?;
        Ok(Client::new(&self.character_service).with_token(&token))
    }

    /// The characters of `campaign` that the player device `device` (a key)
    /// plays.
    pub async fn played_by(
        &self,
        campaign: Uuid,
        device: &str,
    ) -> Result<HashSet<Uuid>, ClientError> {
        let characters = self.characters(campaign).await?.list().await?;
        Ok(characters
            .into_iter()
            .filter(|c| c.owners.iter().any(|owner| owner == device))
            .map(|c| c.id)
            .collect())
    }

    /// Spends `cost` on `character`'s sheet for `command`, once however
    /// often the command is sent.
    pub async fn spend(
        &self,
        campaign: Uuid,
        character: Uuid,
        cost: &SheetCost,
        command: Uuid,
    ) -> Result<(), ClientError> {
        let characters = self.characters(campaign).await?;
        characters
            .act_once(character, &cost.spend(), command)
            .await
            .map(drop)
    }

    /// Gives `cost` back to `character`'s sheet, once per `key`.
    pub async fn give_back(
        &self,
        campaign: Uuid,
        character: Uuid,
        cost: &SheetCost,
        key: Uuid,
    ) -> Result<(), ClientError> {
        let characters = self.characters(campaign).await?;
        match characters.act_once(character, &cost.give_back(), key).await {
            Ok(_) | Err(ClientError::NotFound) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Syncs `combat` until it is up to date, unless another instance is
    /// already on it (that instance then picks up anything new too).
    pub async fn run(self: Arc<Self>, combat: Uuid) {
        loop {
            match self.sync_once(combat).await {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    tracing::warn!(%combat, error = %e, "hit point write-back failed; will retry");
                    break;
                }
            }
        }
    }

    pub async fn retry_forever(self: Arc<Self>) {
        let mut interval = tokio::time::interval(RETRY_INTERVAL);
        loop {
            interval.tick().await;
            let behind: Result<Vec<(Uuid,)>, _> = sqlx::query_as(
                "SELECT s.combat_id FROM combat_hp_sync s JOIN combats c ON c.id = s.combat_id \
                 WHERE s.synced_seq < c.seq",
            )
            .fetch_all(self.store.pool())
            .await;
            match behind {
                Ok(combats) => {
                    for (combat,) in combats {
                        self.clone().run(combat).await;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "finding combats to sync failed"),
            }
        }
    }

    /// One pass: writes back every HP change since the last sync. `true` if
    /// it made progress (so there may be more to do), `false` if there was
    /// nothing to do or another instance holds the combat.
    async fn sync_once(&self, combat: Uuid) -> Result<bool, SyncError> {
        let mut tx = self.store.begin().await?;
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT synced_seq FROM combat_hp_sync WHERE combat_id = $1 FOR UPDATE SKIP LOCKED",
        )
        .bind(combat)
        .fetch_optional(&mut *tx)
        .await
        .map_err(ApiError::from)?;
        let Some((synced,)) = row else {
            return Ok(false);
        };
        let synced = u64::try_from(synced).unwrap_or(0);
        let log = self.store.events_in(&mut tx, combat, 0).await?;
        let Some(latest) = log.last().map(|e| e.seq).filter(|&seq| seq > synced) else {
            return Ok(false);
        };

        let changes = hp_changes(&log, synced);
        let refunds = undone_costs(&log, synced);
        let campaign = CombatState::replay(log.iter().map(|e| &e.event)).campaign;
        if !changes.is_empty() || !refunds.is_empty() {
            match campaign.parse() {
                Ok(campaign) => {
                    self.write(combat, campaign, &changes).await?;
                    for (character, cost, command) in &refunds {
                        self.give_back(campaign, *character, cost, undone_key(*command))
                            .await?;
                    }
                }
                Err(_) => {
                    tracing::warn!(%combat, %campaign, "combat has no valid campaign; not syncing");
                }
            }
        }

        sqlx::query("UPDATE combat_hp_sync SET synced_seq = $1 WHERE combat_id = $2")
            .bind(i64::try_from(latest).unwrap_or(i64::MAX))
            .bind(combat)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::from)?;
        tx.commit().await.map_err(ApiError::from)?;
        Ok(true)
    }

    async fn write(
        &self,
        combat: Uuid,
        campaign: Uuid,
        changes: &HashMap<Uuid, HitPoints>,
    ) -> Result<(), SyncError> {
        let characters = self.characters(campaign).await?;
        for (&character, &hp) in changes {
            let written = characters
                .modify(character, |sheet| {
                    let sheet_hp = &mut sheet.hit_points;
                    // The sheet's maximum may have changed since the
                    // character joined the combat.
                    sheet_hp.current = hp.current.min(sheet_hp.max);
                    sheet_hp.temporary = hp.temporary;
                    Ok::<_, ClientError>(())
                })
                .await;
            match written {
                Ok(_) => {}
                Err(ClientError::NotFound) => {
                    tracing::info!(%combat, %character, "character no longer exists; skipping");
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}

/// The latest hit points of every character whose HP changed after log
/// position `after`, keyed by character id. Joining a combat isn't a
/// change: a combatant starts with the sheet's own HP.
fn hp_changes(log: &[LoggedEvent], after: u64) -> HashMap<Uuid, HitPoints> {
    let mut state = CombatState::default();
    let mut known: HashMap<Uuid, HitPoints> = HashMap::new();
    let mut changes = HashMap::new();
    for event in log {
        state.apply(&event.event);
        for combatant in &state.combatants {
            let Some(character) = combatant.character_id else {
                continue;
            };
            let previous = known.insert(combatant.id, combatant.hit_points);
            if event.seq > after && previous.is_some_and(|hp| hp != combatant.hit_points) {
                changes.insert(character, combatant.hit_points);
            }
        }
    }
    changes
}

/// What the commands undone after log position `after` had spent on
/// characters' sheets: the character, the cost, and the undone command.
fn undone_costs(log: &[LoggedEvent], after: u64) -> Vec<(Uuid, SheetCost, Uuid)> {
    // Combatants may have left by the time their command is undone.
    let mut characters: HashMap<Uuid, Uuid> = HashMap::new();
    for event in log {
        if let CombatEvent::CombatantAdded { combatant }
        | CombatEvent::CombatantRestored { combatant } = &event.event
            && let Some(character) = combatant.character_id
        {
            characters.insert(combatant.id, character);
        }
    }
    log.iter()
        .filter(|e| e.seq > after)
        .filter_map(|e| match e.event {
            CombatEvent::Undone { command } => Some(command),
            _ => None,
        })
        .flat_map(|command| {
            log.iter()
                .filter(move |e| e.command_id == Some(command))
                .filter_map(|e| e.event.sheet_cost())
                .filter_map(|(combatant, cost)| {
                    characters.get(&combatant).map(|c| (*c, cost, command))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use edda_types::combat::{CombatEvent, Combatant};

    use super::*;

    fn logged(seq: u64, event: CombatEvent) -> LoggedEvent {
        LoggedEvent {
            seq,
            command_id: None,
            actor: None,
            at: Utc::now(),
            event,
        }
    }

    fn hp(current: u16, temporary: u16) -> HitPoints {
        HitPoints {
            max: 30,
            current,
            temporary,
        }
    }

    #[test]
    fn only_changes_after_the_sync_point_count_and_the_latest_wins() {
        let character = Uuid::new_v4();
        let mut vex = Combatant::new("Vex", 30);
        vex.character_id = Some(character);
        let goblin = Combatant::new("Goblin", 7);
        let damaged = |seq, combatant, current| {
            logged(
                seq,
                CombatEvent::Damaged {
                    combatant,
                    amount: 1,
                    hit_points: hp(current, 0),
                },
            )
        };
        let log = vec![
            logged(
                1,
                CombatEvent::Created {
                    name: "A".into(),
                    campaign: "C".into(),
                },
            ),
            logged(
                2,
                CombatEvent::CombatantAdded {
                    combatant: vex.clone(),
                },
            ),
            logged(
                3,
                CombatEvent::CombatantAdded {
                    combatant: goblin.clone(),
                },
            ),
            damaged(4, vex.id, 25),
            damaged(5, goblin.id, 2),
            damaged(6, vex.id, 20),
        ];
        assert!(hp_changes(&log, 3).len() == 1);
        assert_eq!(hp_changes(&log, 3)[&character], hp(20, 0));
        assert_eq!(hp_changes(&log, 5)[&character], hp(20, 0));
        assert!(hp_changes(&log, 6).is_empty());
        assert!(
            hp_changes(&log[..3], 0).is_empty(),
            "joining isn't a change"
        );

        // A character who leaves keeps the damage taken before leaving.
        let mut log = log;
        log.push(damaged(7, vex.id, 10));
        log.push(logged(
            8,
            CombatEvent::CombatantRemoved { combatant: vex.id },
        ));
        assert_eq!(hp_changes(&log, 6)[&character], hp(10, 0));
    }
}
