//! A campaign's combats, logs and all, and its planned encounters,
//! exported to keep or to move to another server, and imported there.

use edda_types::{
    admin::{CombatExport, ExportedCombat, SentCommand},
    combat::{CombatEvent, CombatState, LoggedEvent},
    encounter::Encounter,
};
use sqlx::types::Json;
use uuid::Uuid;

use crate::{
    encounters,
    error::ApiError,
    store::{self, Store, Tx},
};

impl Store {
    /// The campaign's combats and plans, as they are.
    pub async fn export_campaign(&self, campaign: Uuid) -> Result<CombatExport, ApiError> {
        let ids: Vec<(Uuid,)> =
            sqlx::query_as("SELECT id FROM combats WHERE campaign = $1 ORDER BY created_at, id")
                .bind(campaign.to_string())
                .fetch_all(self.pool())
                .await?;
        let mut combats = Vec::with_capacity(ids.len());
        for (id,) in ids {
            combats.push(self.export_combat(id).await?);
        }
        let rows: Vec<encounters::Row> = sqlx::query_as(
            "SELECT id, campaign, plan, version, created_at, updated_at FROM encounters \
             WHERE campaign = $1 ORDER BY created_at, id",
        )
        .bind(campaign.to_string())
        .fetch_all(self.pool())
        .await?;
        let encounters = rows
            .into_iter()
            .map(encounters::from_row)
            .collect::<Result<_, _>>()?;
        Ok(CombatExport {
            combats,
            encounters,
        })
    }

    async fn export_combat(&self, id: Uuid) -> Result<ExportedCombat, ApiError> {
        let log = self.events(id, 0).await?.unwrap_or_default();
        let commands: Vec<(Uuid, Option<String>)> = sqlx::query_as(
            "SELECT command_id, actor_key FROM combat_commands WHERE combat_id = $1 \
             ORDER BY command_id",
        )
        .bind(id)
        .fetch_all(self.pool())
        .await?;
        let synced: Option<(i64,)> =
            sqlx::query_as("SELECT synced_seq FROM combat_hp_sync WHERE combat_id = $1")
                .bind(id)
                .fetch_optional(self.pool())
                .await?;
        Ok(ExportedCombat {
            id,
            log,
            commands: commands
                .into_iter()
                .map(|(id, sender)| SentCommand { id, sender })
                .collect(),
            hp_synced: synced.map(|(seq,)| store::to_seq(seq)).transpose()?,
        })
    }

    /// Takes in a campaign's combats and plans exported elsewhere, as they
    /// were; returns how many came. All or nothing, and only while it has
    /// none here. Each combat's log must be whole, from its creation in
    /// this campaign on; the rest follows from it.
    pub async fn import_campaign(
        &self,
        campaign: Uuid,
        export: &CombatExport,
    ) -> Result<u64, ApiError> {
        let name = campaign.to_string();
        if export.encounters.iter().any(|e| e.campaign != campaign) {
            return Err(ApiError::BadRequest(
                "a plan of another campaign is in the export".into(),
            ));
        }
        let mut tx = self.begin().await?;
        let (kept,): (i64,) = sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM combats WHERE campaign = $1) \
             + (SELECT COUNT(*) FROM encounters WHERE campaign = $1)",
        )
        .bind(&name)
        .fetch_one(&mut *tx)
        .await?;
        if kept > 0 {
            return Err(ApiError::Conflict(format!(
                "campaign {campaign} has {kept} combat(s) or plan(s) here already"
            )));
        }
        for combat in &export.combats {
            import_combat(&mut tx, &name, combat).await?;
        }
        for encounter in &export.encounters {
            import_encounter(&mut tx, encounter).await?;
        }
        tx.commit().await?;
        let count = export.combats.len() + export.encounters.len();
        Ok(u64::try_from(count).unwrap_or(u64::MAX))
    }
}

async fn import_combat(
    tx: &mut Tx,
    campaign: &str,
    combat: &ExportedCombat,
) -> Result<(), ApiError> {
    let whole = |log: &[LoggedEvent]| log.iter().zip(1..).all(|(event, seq)| event.seq == seq);
    let (Some(first), Some(last)) = (combat.log.first(), combat.log.last()) else {
        return Err(ApiError::BadRequest(format!(
            "combat {} has no log",
            combat.id
        )));
    };
    let created_here =
        matches!(&first.event, CombatEvent::Created { campaign: c, .. } if c == campaign);
    if !whole(&combat.log) || !created_here {
        return Err(ApiError::BadRequest(format!(
            "combat {}'s log isn't whole, from its creation in this campaign",
            combat.id
        )));
    }
    if combat.hp_synced.is_some_and(|seq| seq > last.seq) {
        return Err(ApiError::BadRequest(format!(
            "combat {}'s sheets can't be ahead of its log",
            combat.id
        )));
    }
    let state = CombatState::replay(combat.log.iter().map(|e| &e.event));
    sqlx::query(
        "INSERT INTO combats (id, campaign, name, status, round, seq, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(combat.id)
    .bind(campaign)
    .bind(&state.name)
    .bind(store::status_str(state.status))
    .bind(i32::try_from(state.round).map_err(|e| ApiError::BadRequest(e.to_string()))?)
    .bind(store::from_seq(last.seq)?)
    .bind(first.at)
    .bind(last.at)
    .execute(&mut **tx)
    .await?;
    for event in &combat.log {
        store::insert_event(tx, combat.id, event).await?;
    }
    for command in &combat.commands {
        sqlx::query(
            "INSERT INTO combat_commands (combat_id, command_id, actor_key) VALUES ($1, $2, $3)",
        )
        .bind(combat.id)
        .bind(command.id)
        .bind(&command.sender)
        .execute(&mut **tx)
        .await?;
    }
    if let Some(seq) = combat.hp_synced {
        sqlx::query("INSERT INTO combat_hp_sync (combat_id, synced_seq) VALUES ($1, $2)")
            .bind(combat.id)
            .bind(store::from_seq(seq)?)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn import_encounter(tx: &mut Tx, encounter: &Encounter) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO encounters (id, campaign, name, plan, version, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(encounter.id)
    .bind(encounter.campaign.to_string())
    .bind(&encounter.plan.name)
    .bind(Json(&encounter.plan))
    .bind(i64::try_from(encounter.version).map_err(|e| ApiError::BadRequest(e.to_string()))?)
    .bind(encounter.created_at)
    .bind(encounter.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
