//! Moving the characters from SQLite, where the service used to keep them,
//! into Postgres: `character-service import-sqlite <FILE>`.

use std::{collections::HashSet, error::Error, path::Path};

use chrono::{DateTime, Utc};
use edda_types::Character;
use sqlx::{
    PgPool, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::store;

/// What an import brought over.
#[derive(Debug, PartialEq, Eq)]
pub struct Imported {
    pub characters: usize,
    /// Characters from before campaigns existed, which no campaign could
    /// see, left behind.
    pub without_campaign: usize,
}

type Row = (
    String,
    Option<String>,
    String,
    String,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// Copies every character in the SQLite database `file` into `pool`, as it
/// is: id, campaign, owners, version and dates included, so everything
/// that refers to them still does. All or nothing, and only into a
/// Postgres that has no characters yet, so it can't clobber or double up.
pub async fn import_sqlite(pool: &PgPool, file: &Path) -> Result<Imported, Box<dyn Error>> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM characters")
        .fetch_one(pool)
        .await?;
    if count > 0 {
        return Err(
            format!("Postgres already has {count} character(s); not importing over them").into(),
        );
    }
    let sqlite = open(file).await?;
    // Databases from before owners existed have no such column.
    let columns: HashSet<String> =
        sqlx::query_as("SELECT name FROM pragma_table_info('characters')")
            .fetch_all(&sqlite)
            .await?
            .into_iter()
            .map(|(name,): (String,)| name)
            .collect();
    let query = if columns.contains("owners") {
        "SELECT id, campaign, owners, sheet, version, created_at, updated_at FROM characters"
    } else {
        "SELECT id, campaign, '[]', sheet, version, created_at, updated_at FROM characters"
    };
    let rows: Vec<Row> = sqlx::query_as(query).fetch_all(&sqlite).await?;

    let mut imported = Imported {
        characters: 0,
        without_campaign: 0,
    };
    let mut tx = pool.begin().await?;
    for (id, campaign, owners, sheet, version, created_at, updated_at) in rows {
        let Some(campaign) = campaign else {
            imported.without_campaign += 1;
            continue;
        };
        let character = Character {
            id: id.parse()?,
            campaign: campaign.parse()?,
            owners: serde_json::from_str(&owners)?,
            sheet: serde_json::from_str(&sheet)?,
            version: u64::try_from(version)?,
            created_at,
            updated_at,
        };
        store::insert(&mut *tx, &character).await?;
        imported.characters += 1;
    }
    tx.commit().await?;
    Ok(imported)
}

/// The SQLite database `file`, only to read.
async fn open(file: &Path) -> Result<SqlitePool, Box<dyn Error>> {
    if !file.exists() {
        return Err(format!("there's no {}", file.display()).into());
    }
    let options = SqliteConnectOptions::new().filename(file).read_only(true);
    Ok(SqlitePoolOptions::new().connect_with(options).await?)
}
