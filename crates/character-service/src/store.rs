use campaign_auth::protocol::{Claims, Principal};
use campaign_types::{Character, CharacterAction, CharacterSheet, Edition};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::ApiError;

type Row = (
    String,
    String,
    String,
    String,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// Whose characters, by which rules, and who's asking: every operation is
/// scoped to one campaign, spell slots follow the classes under its
/// edition, and a player's device changes only the characters it owns.
#[derive(Debug, Clone)]
pub struct Scope {
    pub campaign: Uuid,
    pub edition: Edition,
    /// The player device asking, by key; `None` for a DM device or a
    /// service, which may change any character.
    pub player: Option<String>,
}

impl From<&Claims> for Scope {
    fn from(claims: &Claims) -> Self {
        Self {
            campaign: claims.campaign,
            edition: claims.edition,
            player: (claims.role == Principal::Player).then(|| claims.key.to_string()),
        }
    }
}

impl Scope {
    /// A stored character as it stands under the campaign's rules: sheets
    /// saved before their slots followed the classes show the right ones.
    fn view(&self, mut character: Character) -> Result<Character, ApiError> {
        character.sheet.derive_slots(self.edition);
        Ok(character)
    }

    /// Whether the asker may change `character`: a DM device or a service
    /// may change any, a player's device only one it owns.
    fn may_change(&self, character: &Character) -> Result<(), ApiError> {
        match &self.player {
            Some(key) if !character.owners.contains(key) => Err(ApiError::Forbidden(format!(
                "{} isn't yours; the DM or their player changes them",
                character.sheet.name
            ))),
            _ => Ok(()),
        }
    }
}

/// Persistence for characters, backed by SQLite. Every operation is scoped
/// to one campaign: a character of another campaign is simply not found.
#[derive(Clone)]
pub struct CharacterStore {
    pool: SqlitePool,
}

impl CharacterStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn list(&self, scope: Scope) -> Result<Vec<Character>, ApiError> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, campaign, owners, sheet, version, created_at, updated_at FROM characters \
             WHERE campaign = ? ORDER BY name COLLATE NOCASE",
        )
        .bind(scope.campaign.to_string())
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| scope.view(from_row(row)?))
            .collect()
    }

    pub async fn get(&self, scope: Scope, id: Uuid) -> Result<Character, ApiError> {
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, campaign, owners, sheet, version, created_at, updated_at FROM characters WHERE id = ? AND campaign = ?",
        )
        .bind(id.to_string())
        .bind(scope.campaign.to_string())
        .fetch_optional(&self.pool)
        .await?;
        scope.view(row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?)
    }

    pub async fn create(
        &self,
        scope: Scope,
        mut sheet: CharacterSheet,
    ) -> Result<Character, ApiError> {
        sheet.derive_slots(scope.edition);
        sheet.validate()?;
        let now = Utc::now();
        let character = Character {
            id: Uuid::new_v4(),
            campaign: scope.campaign,
            // A player's device plays what it creates.
            owners: scope.player.iter().cloned().collect(),
            sheet,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        sqlx::query(
            "INSERT INTO characters (id, campaign, owners, name, sheet, version, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(character.id.to_string())
        .bind(scope.campaign.to_string())
        .bind(owners_json(&character.owners)?)
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
        scope: Scope,
        id: Uuid,
        expected_version: Option<u64>,
        mut sheet: CharacterSheet,
    ) -> Result<Character, ApiError> {
        if scope.player.is_some() {
            scope.may_change(&self.get(scope.clone(), id).await?)?;
        }
        sheet.derive_slots(scope.edition);
        sheet.validate()?;
        // SQLite integers are signed; a version beyond i64 can't match any
        // row, and -1 never does either since versions start at 1.
        let expected = expected_version.map(|v| i64::try_from(v).unwrap_or(-1));
        let row: Option<Row> = sqlx::query_as(
            "UPDATE characters SET name = ?, sheet = ?, updated_at = ?, version = version + 1 \
             WHERE id = ? AND campaign = ? AND version = COALESCE(?, version) \
             RETURNING id, campaign, owners, sheet, version, created_at, updated_at",
        )
        .bind(&sheet.name)
        .bind(to_json(&sheet)?)
        .bind(Utc::now())
        .bind(id.to_string())
        .bind(scope.campaign.to_string())
        .bind(expected)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => from_row(row),
            // Either the character doesn't exist (get returns NotFound) or
            // the version moved on; hand back the current state to merge onto.
            None => Err(ApiError::VersionMismatch(Box::new(
                self.get(scope, id).await?,
            ))),
        }
    }

    /// Applies an in-play action atomically. `BEGIN IMMEDIATE` takes the
    /// write lock up front so concurrent actions (e.g. two clients spending
    /// the same slot) serialize instead of overwriting each other.
    /// Applies `action` to the sheet. With a `key`, only once: sent again
    /// with the same key, the character is returned as it now is.
    pub async fn apply(
        &self,
        scope: Scope,
        id: Uuid,
        action: &CharacterAction,
        key: Option<&str>,
    ) -> Result<Character, ApiError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let row: Option<Row> = sqlx::query_as(
            "SELECT id, campaign, owners, sheet, version, created_at, updated_at FROM characters WHERE id = ? AND campaign = ?",
        )
        .bind(id.to_string())
        .bind(scope.campaign.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let mut character = row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?;
        scope.may_change(&character)?;
        if let Some(key) = key {
            let seen: Option<(i64,)> =
                sqlx::query_as("SELECT 1 FROM applied_actions WHERE character = ? AND key = ?")
                    .bind(id.to_string())
                    .bind(key)
                    .fetch_optional(&mut *tx)
                    .await?;
            if seen.is_some() {
                return Ok(character);
            }
            sqlx::query(
                "INSERT INTO applied_actions (character, key, applied_at) VALUES (?, ?, ?)",
            )
            .bind(id.to_string())
            .bind(key)
            .bind(Utc::now())
            .execute(&mut *tx)
            .await?;
        }

        // Sheets saved before their slots followed the classes catch up here.
        character.sheet.derive_slots(scope.edition);
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

    /// Sets which player devices play the character, by key. That's a
    /// change like any other: the version moves on.
    pub async fn set_owners(
        &self,
        scope: Scope,
        id: Uuid,
        owners: &[String],
    ) -> Result<Character, ApiError> {
        let row: Option<Row> = sqlx::query_as(
            "UPDATE characters SET owners = ?, updated_at = ?, version = version + 1 \
             WHERE id = ? AND campaign = ? \
             RETURNING id, campaign, owners, sheet, version, created_at, updated_at",
        )
        .bind(owners_json(owners)?)
        .bind(Utc::now())
        .bind(id.to_string())
        .bind(scope.campaign.to_string())
        .fetch_optional(&self.pool)
        .await?;
        scope.view(row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?)
    }

    /// Deletes every character of a campaign; returns how many there were.
    pub async fn delete_campaign(&self, campaign: Uuid) -> Result<u64, ApiError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "DELETE FROM applied_actions WHERE character IN \
             (SELECT id FROM characters WHERE campaign = ?)",
        )
        .bind(campaign.to_string())
        .execute(&mut *tx)
        .await?;
        let result = sqlx::query("DELETE FROM characters WHERE campaign = ?")
            .bind(campaign.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn delete(&self, scope: Scope, id: Uuid) -> Result<(), ApiError> {
        if scope.player.is_some() {
            scope.may_change(&self.get(scope.clone(), id).await?)?;
        }
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query("DELETE FROM characters WHERE id = ? AND campaign = ?")
            .bind(id.to_string())
            .bind(scope.campaign.to_string())
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound);
        }
        sqlx::query("DELETE FROM applied_actions WHERE character = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

fn owners_json(owners: &[String]) -> Result<String, ApiError> {
    serde_json::to_string(owners).map_err(|e| ApiError::Corrupt(e.to_string()))
}

fn to_json(sheet: &CharacterSheet) -> Result<String, ApiError> {
    serde_json::to_string(sheet).map_err(|e| ApiError::Corrupt(e.to_string()))
}

fn from_row(
    (id, campaign, owners, sheet, version, created_at, updated_at): Row,
) -> Result<Character, ApiError> {
    let parse = |id: &str| {
        id.parse()
            .map_err(|e: uuid::Error| ApiError::Corrupt(e.to_string()))
    };
    Ok(Character {
        id: parse(&id)?,
        campaign: parse(&campaign)?,
        owners: serde_json::from_str(&owners).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        sheet: serde_json::from_str(&sheet).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}
