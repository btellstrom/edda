//! Writes combatants' hit points back to their character sheets.
//!
//! The combat log stays the authority during a fight; the character service
//! follows it. How far each combat has been written back is stored in
//! `combat_hp_sync`, so a write-back that fails (the character service is
//! down, say) is retried later by whichever instance gets there first.
//! Only one instance syncs a given combat at a time, and it always writes
//! the latest HP, so sheets never go backwards.

use std::{collections::HashMap, sync::Arc, time::Duration};

use campaign_client::{Client, ClientError};
use campaign_types::{
    HitPoints,
    combat::{CombatState, LoggedEvent},
};
use uuid::Uuid;

use crate::{error::ApiError, store::Store};

/// How often combats whose write-back failed are retried.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

pub struct HpSync {
    store: Store,
    characters: Client,
}

#[derive(Debug, thiserror::Error)]
enum SyncError {
    #[error(transparent)]
    Store(#[from] ApiError),
    #[error("character service: {0}")]
    Characters(#[from] ClientError),
}

impl HpSync {
    pub fn new(store: Store, characters: Client) -> Self {
        Self { store, characters }
    }

    /// Syncs `combat` until it is up to date, unless another instance is
    /// already on it (that instance then picks up anything new too).
    pub async fn run(self: Arc<Self>, combat: Uuid) {
        loop {
            match self.sync_once(combat).await {
                Ok(true) => continue,
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

        for (character, hp) in hp_changes(&log, synced) {
            let written = self
                .characters
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

        sqlx::query("UPDATE combat_hp_sync SET synced_seq = $1 WHERE combat_id = $2")
            .bind(i64::try_from(latest).unwrap_or(i64::MAX))
            .bind(combat)
            .execute(&mut *tx)
            .await
            .map_err(ApiError::from)?;
        tx.commit().await.map_err(ApiError::from)?;
        Ok(true)
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

#[cfg(test)]
mod tests {
    use campaign_types::combat::{CombatEvent, Combatant};
    use chrono::Utc;

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
