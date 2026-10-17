//! Persistence for rules text (spells and conditions), alongside the
//! monsters and shared and scoped the same way.

use campaign_types::rules_text::{Entry, RulesText};
use chrono::{DateTime, Utc};
use sqlx::SqliteExecutor;
use uuid::Uuid;

use crate::{
    MonsterStore,
    error::ApiError,
    store::{Imported, Scope},
};

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
        "id, campaign, body, version, created_at, updated_at"
    };
}

impl MonsterStore {
    /// The shared ones suiting the campaign's edition and the campaign's
    /// own, by name.
    pub async fn texts<T: RulesText>(&self, scope: Scope) -> Result<Vec<Entry<T>>, ApiError> {
        let rows: Vec<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM rules_text WHERE kind = ? AND (campaign = ? \
             OR (campaign IS NULL AND (edition IS NULL OR edition = ?))) \
             ORDER BY name COLLATE NOCASE, campaign IS NULL"
        ))
        .bind(T::KIND)
        .bind(scope.campaign.to_string())
        .bind(scope.edition.to_string())
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(from_row).collect()
    }

    /// A shared one (of any edition) or one of the campaign's own.
    pub async fn text<T: RulesText>(&self, scope: Scope, id: Uuid) -> Result<Entry<T>, ApiError> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            columns!(),
            " FROM rules_text WHERE kind = ? AND id = ? AND (campaign IS NULL OR campaign = ?)"
        ))
        .bind(T::KIND)
        .bind(id.to_string())
        .bind(scope.campaign.to_string())
        .fetch_optional(self.pool())
        .await?;
        row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
    }

    /// Adds one to the campaign's own.
    /// How many texts of this kind of its own `campaign` keeps.
    pub async fn count_own_texts<T: RulesText>(&self, campaign: Uuid) -> Result<i64, ApiError> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM rules_text WHERE kind = ? AND campaign = ?")
                .bind(T::KIND)
                .bind(campaign.to_string())
                .fetch_one(self.pool())
                .await?;
        Ok(count)
    }

    pub async fn create_text<T: RulesText>(
        &self,
        scope: Scope,
        text: T,
    ) -> Result<Entry<T>, ApiError> {
        text.validate()?;
        insert(self.pool(), Some(scope.campaign), &text).await
    }

    /// Replaces one of the campaign's own.
    pub async fn replace_text<T: RulesText>(
        &self,
        scope: Scope,
        id: Uuid,
        text: T,
    ) -> Result<Entry<T>, ApiError> {
        text.validate()?;
        self.owned_text::<T>(scope, id).await?;
        update(self.pool(), id, &text).await
    }

    /// Deletes one of the campaign's own.
    pub async fn delete_text<T: RulesText>(&self, scope: Scope, id: Uuid) -> Result<(), ApiError> {
        self.owned_text::<T>(scope, id).await?;
        sqlx::query("DELETE FROM rules_text WHERE id = ?")
            .bind(id.to_string())
            .execute(self.pool())
            .await?;
        Ok(())
    }

    async fn owned_text<T: RulesText>(&self, scope: Scope, id: Uuid) -> Result<(), ApiError> {
        match self.text::<T>(scope, id).await?.campaign {
            Some(_) => Ok(()),
            None => Err(ApiError::Shared),
        }
    }

    /// Like [`MonsterStore::import`]: into the shared library or a
    /// campaign, updating ones with the same name, source and edition; all
    /// or nothing.
    pub async fn import_texts<T: RulesText>(
        &self,
        campaign: Option<Uuid>,
        texts: &[T],
    ) -> Result<Imported, ApiError> {
        for text in texts {
            text.validate()?;
        }
        let mut tx = self.pool().begin().await?;
        let mut imported = Imported::default();
        for text in texts {
            let existing: Option<(String,)> = sqlx::query_as(
                "SELECT id FROM rules_text WHERE kind = ? AND campaign IS ? AND name = ? \
                 AND source = ? AND edition IS ?",
            )
            .bind(T::KIND)
            .bind(campaign.map(|c| c.to_string()))
            .bind(text.name())
            .bind(text.source())
            .bind(text.edition().map(|e| e.to_string()))
            .fetch_optional(&mut *tx)
            .await?;
            match existing {
                Some((id,)) => {
                    update(&mut *tx, parse_id(&id)?, text).await?;
                    imported.updated += 1;
                }
                None => {
                    insert(&mut *tx, campaign, text).await?;
                    imported.added += 1;
                }
            }
        }
        tx.commit().await?;
        Ok(imported)
    }
}

async fn insert<T: RulesText>(
    executor: impl SqliteExecutor<'_>,
    campaign: Option<Uuid>,
    text: &T,
) -> Result<Entry<T>, ApiError> {
    let now = Utc::now();
    let entry = Entry {
        id: Uuid::new_v4(),
        campaign,
        text: text.clone(),
        version: 1,
        created_at: now,
        updated_at: now,
    };
    sqlx::query(
        "INSERT INTO rules_text (id, kind, campaign, name, source, edition, body, version, \
         created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
    )
    .bind(entry.id.to_string())
    .bind(T::KIND)
    .bind(campaign.map(|c| c.to_string()))
    .bind(text.name())
    .bind(text.source())
    .bind(text.edition().map(|e| e.to_string()))
    .bind(to_json(text)?)
    .bind(now)
    .bind(now)
    .execute(executor)
    .await?;
    Ok(entry)
}

async fn update<T: RulesText>(
    executor: impl SqliteExecutor<'_>,
    id: Uuid,
    text: &T,
) -> Result<Entry<T>, ApiError> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE rules_text SET name = ?, source = ?, edition = ?, body = ?, \
         version = version + 1, updated_at = ? WHERE id = ? RETURNING ",
        columns!()
    ))
    .bind(text.name())
    .bind(text.source())
    .bind(text.edition().map(|e| e.to_string()))
    .bind(to_json(text)?)
    .bind(Utc::now())
    .bind(id.to_string())
    .fetch_optional(executor)
    .await?;
    row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
}

fn to_json<T: RulesText>(text: &T) -> Result<String, ApiError> {
    serde_json::to_string(text).map_err(|e| ApiError::Corrupt(e.to_string()))
}

fn parse_id(id: &str) -> Result<Uuid, ApiError> {
    id.parse()
        .map_err(|e: uuid::Error| ApiError::Corrupt(e.to_string()))
}

fn from_row<T: RulesText>(
    (id, campaign, body, version, created_at, updated_at): Row,
) -> Result<Entry<T>, ApiError> {
    Ok(Entry {
        id: parse_id(&id)?,
        campaign: campaign.as_deref().map(parse_id).transpose()?,
        text: serde_json::from_str(&body).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}
