//! Persistence for monsters, backed by SQLite.

use campaign_auth::protocol::Claims;
use campaign_types::{
    Edition,
    monster::{Monster, MonsterSheet},
};
use chrono::{DateTime, Utc};
use sqlx::{SqliteExecutor, SqlitePool};
use uuid::Uuid;

use crate::error::ApiError;

type Row = (
    String,
    Option<String>,
    String,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// The columns a [`Row`] is read from; a macro so queries stay `'static`.
macro_rules! columns {
    () => {
        "id, campaign, sheet, version, created_at, updated_at"
    };
}

/// Whose monsters a request may see and change: the shared library's (of
/// its edition) and its campaign's own.
#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub campaign: Uuid,
    pub edition: Edition,
}

impl From<&Claims> for Scope {
    fn from(claims: &Claims) -> Self {
        Self {
            campaign: claims.campaign,
            edition: claims.edition,
        }
    }
}

/// What an import did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Imported {
    pub added: usize,
    pub updated: usize,
}

#[derive(Clone)]
pub struct MonsterStore {
    pool: SqlitePool,
}

impl MonsterStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The shared monsters suiting the campaign's edition and the
    /// campaign's own, by name.
    pub async fn list(&self, scope: Scope) -> Result<Vec<Monster>, ApiError> {
        let rows: Vec<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM monsters \
             WHERE campaign = ? OR (campaign IS NULL AND (edition IS NULL OR edition = ?)) \
             ORDER BY name COLLATE NOCASE, campaign IS NULL"
        ))
        .bind(scope.campaign.to_string())
        .bind(scope.edition.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(from_row).collect()
    }

    /// A shared monster (of any edition) or one of the campaign's own.
    pub async fn get(&self, scope: Scope, id: Uuid) -> Result<Monster, ApiError> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM monsters WHERE id = ? AND (campaign IS NULL OR campaign = ?)"
        ))
        .bind(id.to_string())
        .bind(scope.campaign.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
    }

    /// Adds a monster to the campaign's own.
    pub async fn create(&self, scope: Scope, sheet: MonsterSheet) -> Result<Monster, ApiError> {
        sheet.validate()?;
        insert(&self.pool, Some(scope.campaign), &sheet).await
    }

    /// Replaces one of the campaign's own monsters.
    pub async fn replace(
        &self,
        scope: Scope,
        id: Uuid,
        sheet: MonsterSheet,
    ) -> Result<Monster, ApiError> {
        sheet.validate()?;
        self.owned(scope, id).await?;
        update(&self.pool, id, &sheet).await
    }

    /// Deletes one of the campaign's own monsters.
    pub async fn delete(&self, scope: Scope, id: Uuid) -> Result<(), ApiError> {
        self.owned(scope, id).await?;
        sqlx::query("DELETE FROM monsters WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fails unless `id` is the campaign's own: not found if it can't see
    /// it, forbidden if it's shared.
    async fn owned(&self, scope: Scope, id: Uuid) -> Result<(), ApiError> {
        match self.get(scope, id).await?.campaign {
            Some(_) => Ok(()),
            None => Err(ApiError::Shared),
        }
    }

    /// Adds stat blocks to the shared library (`campaign` `None`) or a
    /// campaign. One with the same name, source and edition as an existing
    /// one there replaces it, so importing the same file again updates it.
    /// All or nothing: every stat block is checked first.
    pub async fn import(
        &self,
        campaign: Option<Uuid>,
        sheets: &[MonsterSheet],
    ) -> Result<Imported, ApiError> {
        for sheet in sheets {
            sheet.validate()?;
        }
        let mut tx = self.pool.begin().await?;
        let mut imported = Imported::default();
        for sheet in sheets {
            let existing: Option<(String,)> = sqlx::query_as(
                "SELECT id FROM monsters WHERE campaign IS ? AND name = ? AND source = ? \
                 AND edition IS ?",
            )
            .bind(campaign.map(|c| c.to_string()))
            .bind(&sheet.name)
            .bind(&sheet.source)
            .bind(sheet.edition.map(|e| e.to_string()))
            .fetch_optional(&mut *tx)
            .await?;
            match existing {
                Some((id,)) => {
                    update(&mut *tx, parse_id(&id)?, sheet).await?;
                    imported.updated += 1;
                }
                None => {
                    insert(&mut *tx, campaign, sheet).await?;
                    imported.added += 1;
                }
            }
        }
        tx.commit().await?;
        Ok(imported)
    }

    /// Deletes a campaign's own monsters, spells and conditions; returns
    /// how many monsters there were.
    pub async fn delete_campaign(&self, campaign: Uuid) -> Result<u64, ApiError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM rules_text WHERE campaign = ?")
            .bind(campaign.to_string())
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM monsters WHERE campaign = ?")
            .bind(campaign.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

async fn insert(
    executor: impl SqliteExecutor<'_>,
    campaign: Option<Uuid>,
    sheet: &MonsterSheet,
) -> Result<Monster, ApiError> {
    let now = Utc::now();
    let monster = Monster {
        id: Uuid::new_v4(),
        campaign,
        sheet: sheet.clone(),
        version: 1,
        created_at: now,
        updated_at: now,
    };
    sqlx::query(
        "INSERT INTO monsters (id, campaign, name, source, edition, sheet, version, created_at, \
         updated_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?)",
    )
    .bind(monster.id.to_string())
    .bind(campaign.map(|c| c.to_string()))
    .bind(&sheet.name)
    .bind(&sheet.source)
    .bind(sheet.edition.map(|e| e.to_string()))
    .bind(to_json(sheet)?)
    .bind(now)
    .bind(now)
    .execute(executor)
    .await?;
    Ok(monster)
}

async fn update(
    executor: impl SqliteExecutor<'_>,
    id: Uuid,
    sheet: &MonsterSheet,
) -> Result<Monster, ApiError> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE monsters SET name = ?, source = ?, edition = ?, sheet = ?, \
         version = version + 1, updated_at = ? WHERE id = ? RETURNING ",
        columns!()
    ))
    .bind(&sheet.name)
    .bind(&sheet.source)
    .bind(sheet.edition.map(|e| e.to_string()))
    .bind(to_json(sheet)?)
    .bind(Utc::now())
    .bind(id.to_string())
    .fetch_optional(executor)
    .await?;
    row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
}

fn to_json(sheet: &MonsterSheet) -> Result<String, ApiError> {
    serde_json::to_string(sheet).map_err(|e| ApiError::Corrupt(e.to_string()))
}

fn parse_id(id: &str) -> Result<Uuid, ApiError> {
    id.parse()
        .map_err(|e: uuid::Error| ApiError::Corrupt(e.to_string()))
}

fn from_row(
    (id, campaign, sheet, version, created_at, updated_at): Row,
) -> Result<Monster, ApiError> {
    Ok(Monster {
        id: parse_id(&id)?,
        campaign: campaign.as_deref().map(parse_id).transpose()?,
        sheet: serde_json::from_str(&sheet).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}
