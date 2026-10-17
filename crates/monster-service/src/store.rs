//! Persistence for monsters, in Postgres.

use campaign_auth::protocol::Claims;
use campaign_types::{
    Edition,
    monster::{Monster, MonsterSheet},
};
use chrono::{DateTime, SubsecRound, Utc};
use sqlx::{PgExecutor, PgPool, types::Json};
use uuid::Uuid;

use crate::error::ApiError;

type Row = (
    Uuid,
    Option<Uuid>,
    Json<MonsterSheet>,
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
    pool: PgPool,
}

impl MonsterStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The shared monsters suiting the campaign's edition and the
    /// campaign's own, by name (the campaign's first, where both have one).
    pub async fn list(&self, scope: Scope) -> Result<Vec<Monster>, ApiError> {
        let rows: Vec<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM monsters \
             WHERE campaign = $1 OR (campaign IS NULL AND (edition IS NULL OR edition = $2)) \
             ORDER BY lower(name), campaign IS NULL"
        ))
        .bind(scope.campaign)
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
            " FROM monsters WHERE id = $1 AND (campaign IS NULL OR campaign = $2)"
        ))
        .bind(id)
        .bind(scope.campaign)
        .fetch_optional(&self.pool)
        .await?;
        row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
    }

    /// How many monsters of its own `campaign` keeps.
    pub async fn count_own(&self, campaign: Uuid) -> Result<i64, ApiError> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM monsters WHERE campaign = $1")
            .bind(campaign)
            .fetch_one(&self.pool)
            .await?;
        Ok(count)
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
        sqlx::query("DELETE FROM monsters WHERE id = $1")
            .bind(id)
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
            let existing: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM monsters WHERE campaign IS NOT DISTINCT FROM $1 AND name = $2 \
                 AND source = $3 AND edition IS NOT DISTINCT FROM $4",
            )
            .bind(campaign)
            .bind(&sheet.name)
            .bind(&sheet.source)
            .bind(sheet.edition.map(|e| e.to_string()))
            .fetch_optional(&mut *tx)
            .await?;
            match existing {
                Some((id,)) => {
                    update(&mut *tx, id, sheet).await?;
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
        sqlx::query("DELETE FROM rules_text WHERE campaign = $1")
            .bind(campaign)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM monsters WHERE campaign = $1")
            .bind(campaign)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

/// The time, as precisely as Postgres keeps it (microseconds), so what's
/// returned matches what's read back later.
pub(crate) fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

async fn insert(
    executor: impl PgExecutor<'_>,
    campaign: Option<Uuid>,
    sheet: &MonsterSheet,
) -> Result<Monster, ApiError> {
    let now = now();
    let monster = Monster {
        id: Uuid::new_v4(),
        campaign,
        sheet: sheet.clone(),
        version: 1,
        created_at: now,
        updated_at: now,
    };
    insert_as_is(executor, &monster).await?;
    Ok(monster)
}

/// Stores `monster` as it is, id and version included.
pub(crate) async fn insert_as_is(
    executor: impl PgExecutor<'_>,
    monster: &Monster,
) -> Result<(), ApiError> {
    let sheet = &monster.sheet;
    sqlx::query(
        "INSERT INTO monsters (id, campaign, name, source, edition, sheet, version, created_at, \
         updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(monster.id)
    .bind(monster.campaign)
    .bind(&sheet.name)
    .bind(&sheet.source)
    .bind(sheet.edition.map(|e| e.to_string()))
    .bind(Json(sheet))
    .bind(i64::try_from(monster.version).map_err(|e| ApiError::Corrupt(e.to_string()))?)
    .bind(monster.created_at)
    .bind(monster.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

async fn update(
    executor: impl PgExecutor<'_>,
    id: Uuid,
    sheet: &MonsterSheet,
) -> Result<Monster, ApiError> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE monsters SET name = $1, source = $2, edition = $3, sheet = $4, \
         version = version + 1, updated_at = $5 WHERE id = $6 RETURNING ",
        columns!()
    ))
    .bind(&sheet.name)
    .bind(&sheet.source)
    .bind(sheet.edition.map(|e| e.to_string()))
    .bind(Json(sheet))
    .bind(now())
    .bind(id)
    .fetch_optional(executor)
    .await?;
    row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
}

fn from_row(
    (id, campaign, Json(sheet), version, created_at, updated_at): Row,
) -> Result<Monster, ApiError> {
    Ok(Monster {
        id,
        campaign,
        sheet,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}
