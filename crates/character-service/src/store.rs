use chrono::{DateTime, SubsecRound, Utc};
use edda_auth::protocol::{Claims, Principal};
use edda_types::{Character, CharacterAction, CharacterSheet, Edition};
use sqlx::{PgPool, types::Json};
use uuid::Uuid;

use crate::error::ApiError;

type Row = (
    Uuid,
    Uuid,
    Vec<String>,
    Json<CharacterSheet>,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
);

/// A character's columns, as [`Row`] reads them.
macro_rules! columns {
    () => {
        "id, campaign, owners, sheet, version, created_at, updated_at"
    };
}

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
    fn view(&self, mut character: Character) -> Character {
        character.sheet.derive_slots(self.edition);
        character
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

/// Persistence for characters, in Postgres. Every operation is scoped to
/// one campaign: a character of another campaign is simply not found.
#[derive(Clone)]
pub struct CharacterStore {
    pool: PgPool,
}

impl CharacterStore {
    /// A store in `pool`'s database.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The campaign's characters.
    pub async fn list(&self, scope: Scope) -> Result<Vec<Character>, ApiError> {
        let rows: Vec<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM characters WHERE campaign = $1 ORDER BY lower(name)"
        ))
        .bind(scope.campaign)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| Ok(scope.view(from_row(row)?)))
            .collect()
    }

    /// How many characters `campaign` keeps.
    pub async fn count(&self, campaign: Uuid) -> Result<i64, ApiError> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM characters WHERE campaign = $1")
                .bind(campaign)
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }

    /// One of the campaign's characters.
    pub async fn get(&self, scope: Scope, id: Uuid) -> Result<Character, ApiError> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM characters WHERE id = $1 AND campaign = $2"
        ))
        .bind(id)
        .bind(scope.campaign)
        .fetch_optional(&self.pool)
        .await?;
        Ok(scope.view(row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?))
    }

    /// Creates a character, played by the device creating it if that's a
    /// player's.
    pub async fn create(
        &self,
        scope: Scope,
        mut sheet: CharacterSheet,
    ) -> Result<Character, ApiError> {
        sheet.derive_slots(scope.edition);
        sheet.validate()?;
        let now = now();
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
        insert(&self.pool, &character).await?;
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
        // Versions are signed in the database; one beyond i64 can't match
        // any row, and -1 never does either since versions start at 1.
        let expected = expected_version.map(|v| i64::try_from(v).unwrap_or(-1));
        let row: Option<Row> = sqlx::query_as(concat!(
            "UPDATE characters SET name = $1, sheet = $2, updated_at = $3, version = version + 1 \
             WHERE id = $4 AND campaign = $5 AND version = COALESCE($6, version) \
             RETURNING ",
            columns!(),
            ""
        ))
        .bind(&sheet.name)
        .bind(Json(&sheet))
        .bind(Utc::now())
        .bind(id)
        .bind(scope.campaign)
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

    /// Applies `action` to the sheet, atomically: the character's row stays
    /// locked until it's written, so concurrent actions (two clients
    /// spending the same slot) take turns. With a `key`, only once: sent
    /// again with the same key, the character is returned as it now is.
    pub async fn apply(
        &self,
        scope: Scope,
        id: Uuid,
        action: &CharacterAction,
        key: Option<&str>,
    ) -> Result<Character, ApiError> {
        let mut tx = self.pool.begin().await?;
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM characters WHERE id = $1 AND campaign = $2 FOR UPDATE"
        ))
        .bind(id)
        .bind(scope.campaign)
        .fetch_optional(&mut *tx)
        .await?;
        let mut character = row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?;
        scope.may_change(&character)?;
        if let Some(key) = key {
            let first = sqlx::query(
                "INSERT INTO applied_actions (character, key, applied_at) VALUES ($1, $2, now()) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(key)
            .execute(&mut *tx)
            .await?
            .rows_affected()
                == 1;
            if !first {
                return Ok(character);
            }
        }

        // Sheets saved before their slots followed the classes catch up here.
        character.sheet.derive_slots(scope.edition);
        character.sheet.apply(action)?;
        character.sheet.validate()?;
        character.version += 1;
        character.updated_at = now();

        sqlx::query(
            "UPDATE characters SET sheet = $1, version = version + 1, updated_at = $2 WHERE id = $3",
        )
        .bind(Json(&character.sheet))
        .bind(character.updated_at)
        .bind(id)
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
        let row: Option<Row> = sqlx::query_as(concat!(
            "UPDATE characters SET owners = $1, updated_at = now(), version = version + 1 \
             WHERE id = $2 AND campaign = $3 RETURNING ",
            columns!(),
            ""
        ))
        .bind(owners)
        .bind(id)
        .bind(scope.campaign)
        .fetch_optional(&self.pool)
        .await?;
        Ok(scope.view(row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?))
    }

    /// Deletes every character of a campaign; returns how many there were.
    /// (Their applied actions go with them.)
    pub async fn delete_campaign(&self, campaign: Uuid) -> Result<u64, ApiError> {
        let result = sqlx::query("DELETE FROM characters WHERE campaign = $1")
            .bind(campaign)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Deletes a character the device may change.
    pub async fn delete(&self, scope: Scope, id: Uuid) -> Result<(), ApiError> {
        if scope.player.is_some() {
            scope.may_change(&self.get(scope.clone(), id).await?)?;
        }
        let result = sqlx::query("DELETE FROM characters WHERE id = $1 AND campaign = $2")
            .bind(id)
            .bind(scope.campaign)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(ApiError::NotFound);
        }
        Ok(())
    }
}

/// The time, as precisely as Postgres keeps it (microseconds), so what's
/// returned matches what's read back later.
fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

/// Stores `character` as it is, id and version included.
pub(crate) async fn insert(
    db: impl sqlx::PgExecutor<'_>,
    character: &Character,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO characters (id, campaign, owners, name, sheet, version, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(character.id)
    .bind(character.campaign)
    .bind(&character.owners)
    .bind(&character.sheet.name)
    .bind(Json(&character.sheet))
    .bind(i64::try_from(character.version).map_err(|e| ApiError::Corrupt(e.to_string()))?)
    .bind(character.created_at)
    .bind(character.updated_at)
    .execute(db)
    .await?;
    Ok(())
}

fn from_row(
    (id, campaign, owners, Json(sheet), version, created_at, updated_at): Row,
) -> Result<Character, ApiError> {
    Ok(Character {
        id,
        campaign,
        owners,
        sheet,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}
