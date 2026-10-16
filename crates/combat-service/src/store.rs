//! Persistence of the combat event log in Postgres, shared by every
//! instance of the service.

use std::collections::HashSet;

use campaign_types::combat::{CombatEvent, CombatState, CombatStatus, CombatSummary, LoggedEvent};
use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgExecutor, PgPool, Postgres, Transaction, types::Json};
use uuid::Uuid;

use crate::error::ApiError;

/// The Postgres channel on which every appended event is announced, so all
/// instances can push it to their connected clients.
pub const CHANNEL: &str = "combat_events";

pub type Tx = Transaction<'static, Postgres>;

type SummaryRow = (
    Uuid,
    String,
    String,
    String,
    i32,
    DateTime<Utc>,
    DateTime<Utc>,
);

type EventRow = (
    i64,
    Option<Uuid>,
    Option<String>,
    DateTime<Utc>,
    Json<CombatEvent>,
);

/// Payload of a notification on [`CHANNEL`]: the combat's log now reaches
/// `seq`. Events themselves are read from the table, which keeps
/// notifications small and lets a listener that missed some catch up.
#[derive(Debug, Serialize, Deserialize)]
pub struct Notice {
    pub combat: Uuid,
    pub seq: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListFilter {
    pub campaign: Option<String>,
    pub status: Option<CombatStatus>,
}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn begin(&self) -> Result<Tx, ApiError> {
        Ok(self.pool.begin().await?)
    }

    /// Creates a combat whose log starts with a `Created` event.
    pub async fn create(&self, campaign: &str, name: &str) -> Result<Uuid, ApiError> {
        let id = Uuid::new_v4();
        let created = LoggedEvent {
            seq: 1,
            command_id: None,
            actor: None,
            at: now(),
            event: CombatEvent::Created {
                name: name.to_string(),
                campaign: campaign.to_string(),
            },
        };
        let mut tx = self.begin().await?;
        sqlx::query(
            "INSERT INTO combats (id, campaign, name, status, round, seq, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, 0, 1, $5, $5)",
        )
        .bind(id)
        .bind(campaign)
        .bind(name)
        .bind(status_str(CombatStatus::Preparing))
        .bind(created.at)
        .execute(&mut *tx)
        .await?;
        insert_event(&mut tx, id, &created).await?;
        sqlx::query("INSERT INTO combat_hp_sync (combat_id, synced_seq) VALUES ($1, 1)")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        notify(&mut tx, id, 1).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Most recently active first.
    pub async fn list(&self, filter: &ListFilter) -> Result<Vec<CombatSummary>, ApiError> {
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT id, campaign, name, status, round, created_at, updated_at FROM combats \
                 WHERE ($1::text IS NULL OR campaign = $1) AND ($2::text IS NULL OR status = $2) \
                 ORDER BY updated_at DESC",
        )
        .bind(&filter.campaign)
        .bind(filter.status.map(status_str))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(id, campaign, name, status, round, created_at, updated_at)| {
                    Ok(CombatSummary {
                        id,
                        campaign,
                        name,
                        status: serde_json::from_value(serde_json::Value::String(status))
                            .map_err(|e| ApiError::Corrupt(e.to_string()))?,
                        round: u32::try_from(round)
                            .map_err(|e| ApiError::Corrupt(e.to_string()))?,
                        created_at,
                        updated_at,
                    })
                },
            )
            .collect()
    }

    /// The combat's log from `after + 1` onwards, or `None` if there's no
    /// such combat.
    pub async fn events(&self, id: Uuid, after: u64) -> Result<Option<Vec<LoggedEvent>>, ApiError> {
        let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM combats WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        if exists.is_none() {
            return Ok(None);
        }
        fetch_events(&self.pool, id, after).await.map(Some)
    }

    /// The events a given command produced.
    pub async fn events_for_command(
        &self,
        id: Uuid,
        command_id: Uuid,
    ) -> Result<Vec<LoggedEvent>, ApiError> {
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT seq, command_id, actor, at, event FROM combat_events \
             WHERE combat_id = $1 AND command_id = $2 ORDER BY seq",
        )
        .bind(id)
        .bind(command_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(from_row).collect()
    }

    /// Locks the combat until `tx` ends, so no other command (on any
    /// instance) can append in the meantime. Returns the latest seq, or
    /// `None` if there's no such combat.
    pub async fn lock(&self, tx: &mut Tx, id: Uuid) -> Result<Option<u64>, ApiError> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT seq FROM combats WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        row.map(|(seq,)| to_seq(seq)).transpose()
    }

    pub async fn events_in(
        &self,
        tx: &mut Tx,
        id: Uuid,
        after: u64,
    ) -> Result<Vec<LoggedEvent>, ApiError> {
        fetch_events(&mut **tx, id, after).await
    }

    /// Records that a command sent by the device with `actor_key` is being
    /// applied. `false` if it already was.
    pub async fn record_command(
        &self,
        tx: &mut Tx,
        id: Uuid,
        command_id: Uuid,
        actor_key: &str,
    ) -> Result<bool, ApiError> {
        let inserted = sqlx::query(
            "INSERT INTO combat_commands (combat_id, command_id, actor_key) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(command_id)
        .bind(actor_key)
        .execute(&mut **tx)
        .await?;
        Ok(inserted.rows_affected() == 1)
    }

    /// The commands to this combat sent by the device with `actor_key`.
    pub async fn commands_by(
        &self,
        tx: &mut Tx,
        id: Uuid,
        actor_key: &str,
    ) -> Result<HashSet<Uuid>, ApiError> {
        let rows: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT command_id FROM combat_commands WHERE combat_id = $1 AND actor_key = $2",
        )
        .bind(id)
        .bind(actor_key)
        .fetch_all(&mut **tx)
        .await?;
        Ok(rows.into_iter().map(|(command,)| command).collect())
    }

    /// Appends events (non-empty, continuing the log), refreshes the
    /// summary and announces them on [`CHANNEL`] once `tx` commits.
    pub async fn append(
        &self,
        tx: &mut Tx,
        id: Uuid,
        events: &[LoggedEvent],
        state: &CombatState,
    ) -> Result<(), ApiError> {
        let Some(last) = events.last() else {
            return Ok(());
        };
        for event in events {
            insert_event(tx, id, event).await?;
        }
        sqlx::query(
            "UPDATE combats SET status = $1, round = $2, seq = $3, updated_at = $4 WHERE id = $5",
        )
        .bind(status_str(state.status))
        .bind(i32::try_from(state.round).map_err(|e| ApiError::Corrupt(e.to_string()))?)
        .bind(from_seq(last.seq)?)
        .bind(last.at)
        .bind(id)
        .execute(&mut **tx)
        .await?;
        notify(tx, id, last.seq).await
    }
}

/// The current time at the precision Postgres stores, so an event read back
/// from the log equals the one that was written.
pub fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

async fn fetch_events(
    executor: impl PgExecutor<'_>,
    id: Uuid,
    after: u64,
) -> Result<Vec<LoggedEvent>, ApiError> {
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT seq, command_id, actor, at, event FROM combat_events \
         WHERE combat_id = $1 AND seq > $2 ORDER BY seq",
    )
    .bind(id)
    .bind(i64::try_from(after).unwrap_or(i64::MAX))
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(from_row).collect()
}

async fn insert_event(tx: &mut Tx, id: Uuid, event: &LoggedEvent) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO combat_events (combat_id, seq, command_id, actor, at, event) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(from_seq(event.seq)?)
    .bind(event.command_id)
    .bind(&event.actor)
    .bind(event.at)
    .bind(Json(&event.event))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Postgres delivers the notification only if and when `tx` commits.
async fn notify(tx: &mut Tx, id: Uuid, seq: u64) -> Result<(), ApiError> {
    let payload = serde_json::to_string(&Notice { combat: id, seq })
        .map_err(|e| ApiError::Corrupt(e.to_string()))?;
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(CHANNEL)
        .bind(payload)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn from_row((seq, command_id, actor, at, Json(event)): EventRow) -> Result<LoggedEvent, ApiError> {
    Ok(LoggedEvent {
        seq: to_seq(seq)?,
        command_id,
        actor,
        at,
        event,
    })
}

fn to_seq(seq: i64) -> Result<u64, ApiError> {
    u64::try_from(seq).map_err(|e| ApiError::Corrupt(e.to_string()))
}

fn from_seq(seq: u64) -> Result<i64, ApiError> {
    i64::try_from(seq).map_err(|e| ApiError::Corrupt(e.to_string()))
}

fn status_str(status: CombatStatus) -> &'static str {
    match status {
        CombatStatus::Preparing => "preparing",
        CombatStatus::Active => "active",
        CombatStatus::Paused => "paused",
        CombatStatus::Ended => "ended",
    }
}
