//! Moving the monsters, spells and conditions from SQLite, where the
//! service used to keep them, into Postgres: `monster-service import-sqlite
//! <FILE>`.

use std::{error::Error, path::Path};

use chrono::{DateTime, Utc};
use edda_types::monster::{Monster, MonsterSheet};
use sqlx::{
    PgPool, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    types::Json,
};

use crate::store;

/// What an import brought over.
#[derive(Debug, PartialEq, Eq)]
pub struct Imported {
    pub monsters: usize,
    /// Spells and conditions.
    pub rules_texts: usize,
}

type MonsterRow = (
    String,
    Option<String>,
    String,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
);

type TextRow = (
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// Copies everything in the SQLite database `file` into `pool`, as it is,
/// ids included, so plans and combats that refer to a monster still do.
/// That includes the shared library, which replaces the one already in
/// Postgres (loaded by the seed job, with ids of its own; the seed job's
/// next run updates the imported ones by name). All or nothing, and not if
/// Postgres has a campaign's own monsters, spells or conditions already.
pub async fn import_sqlite(pool: &PgPool, file: &Path) -> Result<Imported, Box<dyn Error>> {
    let (own,): (i64,) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM monsters WHERE campaign IS NOT NULL) \
         + (SELECT COUNT(*) FROM rules_text WHERE campaign IS NOT NULL)",
    )
    .fetch_one(pool)
    .await?;
    if own > 0 {
        return Err(format!(
            "Postgres already has {own} of the campaigns' own monsters, spells and conditions; \
             not importing over them"
        )
        .into());
    }
    let sqlite = open(file).await?;
    let monsters: Vec<MonsterRow> =
        sqlx::query_as("SELECT id, campaign, sheet, version, created_at, updated_at FROM monsters")
            .fetch_all(&sqlite)
            .await?;
    // Databases from before spells and conditions had none.
    let (has_texts,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rules_text')",
    )
    .fetch_one(&sqlite)
    .await?;
    let texts: Vec<TextRow> = if has_texts {
        sqlx::query_as(
            "SELECT id, kind, campaign, name, source, edition, body, version, created_at, \
             updated_at FROM rules_text",
        )
        .fetch_all(&sqlite)
        .await?
    } else {
        Vec::new()
    };

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM monsters")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM rules_text")
        .execute(&mut *tx)
        .await?;
    for (id, campaign, sheet, version, created_at, updated_at) in &monsters {
        let monster = Monster {
            id: id.parse()?,
            campaign: campaign.as_deref().map(str::parse).transpose()?,
            sheet: serde_json::from_str::<MonsterSheet>(sheet)?,
            version: u64::try_from(*version)?,
            created_at: *created_at,
            updated_at: *updated_at,
        };
        store::insert_as_is(&mut *tx, &monster).await?;
    }
    for (id, kind, campaign, name, source, edition, body, version, created_at, updated_at) in &texts
    {
        sqlx::query(
            "INSERT INTO rules_text (id, kind, campaign, name, source, edition, body, version, \
             created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(id.parse::<uuid::Uuid>()?)
        .bind(kind)
        .bind(
            campaign
                .as_deref()
                .map(str::parse::<uuid::Uuid>)
                .transpose()?,
        )
        .bind(name)
        .bind(source)
        .bind(edition)
        .bind(Json(serde_json::from_str::<serde_json::Value>(body)?))
        .bind(version)
        .bind(created_at)
        .bind(updated_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Imported {
        monsters: monsters.len(),
        rules_texts: texts.len(),
    })
}

/// The SQLite database `file`, only to read.
async fn open(file: &Path) -> Result<SqlitePool, Box<dyn Error>> {
    if !file.exists() {
        return Err(format!("there's no {}", file.display()).into());
    }
    let options = SqliteConnectOptions::new().filename(file).read_only(true);
    Ok(SqlitePoolOptions::new().connect_with(options).await?)
}
