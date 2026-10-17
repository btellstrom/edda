//! Persistence for rules text (spells and conditions), alongside the
//! monsters and shared and scoped the same way.

use chrono::{DateTime, Utc};
use edda_types::rules_text::{Entry, RulesText};
use sqlx::{PgExecutor, types::Json};
use uuid::Uuid;

use crate::{
    MonsterStore,
    error::ApiError,
    store::{Imported, Scope, now},
};

type Row = (
    Uuid,
    Option<Uuid>,
    Json<serde_json::Value>,
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
            " FROM rules_text WHERE kind = $1 AND (campaign = $2 \
             OR (campaign IS NULL AND (edition IS NULL OR edition = $3))) \
             ORDER BY lower(name), campaign IS NULL"
        ))
        .bind(T::KIND)
        .bind(scope.campaign)
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
            " FROM rules_text WHERE kind = $1 AND id = $2 AND (campaign IS NULL OR campaign = $3)"
        ))
        .bind(T::KIND)
        .bind(id)
        .bind(scope.campaign)
        .fetch_optional(self.pool())
        .await?;
        row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
    }

    /// How many texts of this kind of its own `campaign` keeps.
    pub async fn count_own_texts<T: RulesText>(&self, campaign: Uuid) -> Result<i64, ApiError> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM rules_text WHERE kind = $1 AND campaign = $2")
                .bind(T::KIND)
                .bind(campaign)
                .fetch_one(self.pool())
                .await?;
        Ok(count)
    }

    /// Adds one to the campaign's own.
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
        sqlx::query("DELETE FROM rules_text WHERE id = $1")
            .bind(id)
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
            let existing: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM rules_text WHERE kind = $1 AND campaign IS NOT DISTINCT FROM $2 \
                 AND name = $3 AND source = $4 AND edition IS NOT DISTINCT FROM $5",
            )
            .bind(T::KIND)
            .bind(campaign)
            .bind(text.name())
            .bind(text.source())
            .bind(text.edition().map(|e| e.to_string()))
            .fetch_optional(&mut *tx)
            .await?;
            match existing {
                Some((id,)) => {
                    update(&mut *tx, id, text).await?;
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
    executor: impl PgExecutor<'_>,
    campaign: Option<Uuid>,
    text: &T,
) -> Result<Entry<T>, ApiError> {
    let now = now();
    let entry = Entry {
        id: Uuid::new_v4(),
        campaign,
        text: text.clone(),
        version: 1,
        created_at: now,
        updated_at: now,
    };
    insert_as_is(executor, &entry).await?;
    Ok(entry)
}

/// Stores `entry` as it is, id and version included.
pub(crate) async fn insert_as_is<T: RulesText>(
    executor: impl PgExecutor<'_>,
    entry: &Entry<T>,
) -> Result<(), ApiError> {
    let text = &entry.text;
    sqlx::query(
        "INSERT INTO rules_text (id, kind, campaign, name, source, edition, body, version, \
         created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(entry.id)
    .bind(T::KIND)
    .bind(entry.campaign)
    .bind(text.name())
    .bind(text.source())
    .bind(text.edition().map(|e| e.to_string()))
    .bind(Json(text))
    .bind(i64::try_from(entry.version).map_err(|e| ApiError::Corrupt(e.to_string()))?)
    .bind(entry.created_at)
    .bind(entry.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

async fn update<T: RulesText>(
    executor: impl PgExecutor<'_>,
    id: Uuid,
    text: &T,
) -> Result<Entry<T>, ApiError> {
    let row: Option<Row> = sqlx::query_as(concat!(
        "UPDATE rules_text SET name = $1, source = $2, edition = $3, body = $4, \
         version = version + 1, updated_at = $5 WHERE id = $6 RETURNING ",
        columns!()
    ))
    .bind(text.name())
    .bind(text.source())
    .bind(text.edition().map(|e| e.to_string()))
    .bind(Json(text))
    .bind(now())
    .bind(id)
    .fetch_optional(executor)
    .await?;
    row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
}

fn from_row<T: RulesText>(
    (id, campaign, Json(body), version, created_at, updated_at): Row,
) -> Result<Entry<T>, ApiError> {
    Ok(Entry {
        id,
        campaign,
        text: serde_json::from_value(body).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}
