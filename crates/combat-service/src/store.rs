//! Persistence of the combat event log.

use campaign_types::combat::{CombatEvent, CombatState, CombatStatus, CombatSummary, LoggedEvent};
use chrono::{DateTime, Utc};
use sqlx::{Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::error::ApiError;

type EventRow = (i64, Option<String>, Option<String>, DateTime<Utc>, String);

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Creates a combat whose log starts with a `Created` event.
    pub async fn create(&self, name: &str) -> Result<(Uuid, LoggedEvent), ApiError> {
        let id = Uuid::new_v4();
        let created = LoggedEvent {
            seq: 1,
            command_id: None,
            actor: None,
            at: Utc::now(),
            event: CombatEvent::Created {
                name: name.to_string(),
            },
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO combats (id, name, status, round, created_at) VALUES (?, ?, ?, 0, ?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(status_str(CombatStatus::Preparing))
        .bind(created.at)
        .execute(&mut *tx)
        .await?;
        insert_event(&mut tx, id, &created).await?;
        tx.commit().await?;
        Ok((id, created))
    }

    pub async fn list(&self) -> Result<Vec<CombatSummary>, ApiError> {
        let rows: Vec<(String, String, String, i64, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, name, status, round, created_at FROM combats ORDER BY created_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(id, name, status, round, created_at)| {
                Ok(CombatSummary {
                    id: parse_id(&id)?,
                    name,
                    status: serde_json::from_value(serde_json::Value::String(status))
                        .map_err(|e| ApiError::Corrupt(e.to_string()))?,
                    round: u32::try_from(round).map_err(|e| ApiError::Corrupt(e.to_string()))?,
                    created_at,
                })
            })
            .collect()
    }

    /// The combat's log from `after + 1` onwards, or `None` if there's no
    /// such combat.
    pub async fn events(&self, id: Uuid, after: u64) -> Result<Option<Vec<LoggedEvent>>, ApiError> {
        let exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM combats WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        if exists.is_none() {
            return Ok(None);
        }
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT seq, command_id, actor, at, event FROM combat_events \
             WHERE combat_id = ? AND seq > ? ORDER BY seq",
        )
        .bind(id.to_string())
        .bind(i64::try_from(after).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(from_row)
            .collect::<Result<_, _>>()
            .map(Some)
    }

    /// The events a given command produced.
    pub async fn events_for_command(
        &self,
        id: Uuid,
        command_id: Uuid,
    ) -> Result<Vec<LoggedEvent>, ApiError> {
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT seq, command_id, actor, at, event FROM combat_events \
             WHERE combat_id = ? AND command_id = ? ORDER BY seq",
        )
        .bind(id.to_string())
        .bind(command_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(from_row).collect()
    }

    /// Appends events and refreshes the summary, atomically.
    pub async fn append(
        &self,
        id: Uuid,
        events: &[LoggedEvent],
        state: &CombatState,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        for event in events {
            insert_event(&mut tx, id, event).await?;
        }
        sqlx::query("UPDATE combats SET status = ?, round = ? WHERE id = ?")
            .bind(status_str(state.status))
            .bind(i64::from(state.round))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn insert_event(
    tx: &mut Transaction<'_, Sqlite>,
    id: Uuid,
    event: &LoggedEvent,
) -> Result<(), ApiError> {
    let json = serde_json::to_string(&event.event).map_err(|e| ApiError::Corrupt(e.to_string()))?;
    sqlx::query(
        "INSERT INTO combat_events (combat_id, seq, command_id, actor, at, event) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(i64::try_from(event.seq).map_err(|e| ApiError::Corrupt(e.to_string()))?)
    .bind(event.command_id.map(|c| c.to_string()))
    .bind(&event.actor)
    .bind(event.at)
    .bind(json)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn from_row((seq, command_id, actor, at, event): EventRow) -> Result<LoggedEvent, ApiError> {
    Ok(LoggedEvent {
        seq: u64::try_from(seq).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        command_id: command_id.as_deref().map(parse_id).transpose()?,
        actor,
        at,
        event: serde_json::from_str(&event).map_err(|e| ApiError::Corrupt(e.to_string()))?,
    })
}

fn parse_id(id: &str) -> Result<Uuid, ApiError> {
    id.parse()
        .map_err(|e: uuid::Error| ApiError::Corrupt(e.to_string()))
}

fn status_str(status: CombatStatus) -> &'static str {
    match status {
        CombatStatus::Preparing => "preparing",
        CombatStatus::Active => "active",
        CombatStatus::Ended => "ended",
    }
}
