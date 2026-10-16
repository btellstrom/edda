use campaign_types::{Character, CharacterAction, CharacterSheet};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::ApiError;

type Row = (String, String, i64, DateTime<Utc>, DateTime<Utc>);

/// Persistence for characters, backed by SQLite.
#[derive(Clone)]
pub struct CharacterStore {
    pool: SqlitePool,
}

impl CharacterStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self) -> Result<Vec<Character>, ApiError> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, sheet, version, created_at, updated_at FROM characters \
             ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(from_row).collect()
    }

    pub async fn get(&self, id: Uuid) -> Result<Character, ApiError> {
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, sheet, version, created_at, updated_at FROM characters WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
    }

    pub async fn create(&self, sheet: CharacterSheet) -> Result<Character, ApiError> {
        sheet.validate()?;
        let now = Utc::now();
        let character = Character {
            id: Uuid::new_v4(),
            sheet,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        sqlx::query(
            "INSERT INTO characters (id, name, sheet, version, created_at, updated_at) \
             VALUES (?, ?, ?, 1, ?, ?)",
        )
        .bind(character.id.to_string())
        .bind(&character.sheet.name)
        .bind(to_json(&character.sheet)?)
        .bind(character.created_at)
        .bind(character.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(character)
    }

    /// Replaces the sheet if the stored version still equals
    /// `expected_version` (`None` skips the check). The compare and the write
    /// happen in one statement, so there's no window for another write.
    pub async fn replace(
        &self,
        id: Uuid,
        expected_version: Option<u64>,
        sheet: CharacterSheet,
    ) -> Result<Character, ApiError> {
        sheet.validate()?;
        // SQLite integers are signed; a version beyond i64 can't match any
        // row, and -1 never does either since versions start at 1.
        let expected = expected_version.map(|v| i64::try_from(v).unwrap_or(-1));
        let row: Option<Row> = sqlx::query_as(
            "UPDATE characters SET name = ?, sheet = ?, updated_at = ?, version = version + 1 \
             WHERE id = ? AND version = COALESCE(?, version) \
             RETURNING id, sheet, version, created_at, updated_at",
        )
        .bind(&sheet.name)
        .bind(to_json(&sheet)?)
        .bind(Utc::now())
        .bind(id.to_string())
        .bind(expected)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => from_row(row),
            // Either the character doesn't exist (get returns NotFound) or
            // the version moved on; hand back the current state to merge onto.
            None => Err(ApiError::VersionMismatch(Box::new(self.get(id).await?))),
        }
    }

    /// Applies an in-play action atomically. `BEGIN IMMEDIATE` takes the
    /// write lock up front so concurrent actions (e.g. two clients spending
    /// the same slot) serialize instead of overwriting each other.
    pub async fn apply(&self, id: Uuid, action: &CharacterAction) -> Result<Character, ApiError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, sheet, version, created_at, updated_at FROM characters WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let mut character = row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?;

        character.sheet.apply(action)?;
        character.sheet.validate()?;
        character.version += 1;
        character.updated_at = Utc::now();

        sqlx::query(
            "UPDATE characters SET sheet = ?, version = version + 1, updated_at = ? WHERE id = ?",
        )
        .bind(to_json(&character.sheet)?)
        .bind(character.updated_at)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(character)
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), ApiError> {
        let result = sqlx::query("DELETE FROM characters WHERE id = ?")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound);
        }
        Ok(())
    }
}

fn to_json(sheet: &CharacterSheet) -> Result<String, ApiError> {
    serde_json::to_string(sheet).map_err(|e| ApiError::Corrupt(e.to_string()))
}

fn from_row((id, sheet, version, created_at, updated_at): Row) -> Result<Character, ApiError> {
    Ok(Character {
        id: id
            .parse()
            .map_err(|e: uuid::Error| ApiError::Corrupt(e.to_string()))?,
        sheet: serde_json::from_str(&sheet).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}
