use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use campaign_auth::{protocol::Claims, server::Authenticated};
use campaign_types::{
    monster::{Monster, MonsterSheet, MonsterSummary},
    rules_text::{ConditionText, Entry, RulesText, SpellText},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{MonsterStore, error::ApiError};

pub fn router() -> Router<MonsterStore> {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/monsters", get(list).post(create))
        .route("/monsters/{id}", get(fetch).put(replace).delete(remove))
        .merge(texts::<SpellText>())
        .merge(texts::<ConditionText>())
}

/// `/spells` or `/conditions`, like `/monsters` but whole entries in the
/// list (they're short).
fn texts<T: RulesText>() -> Router<MonsterStore> {
    Router::new()
        .route(
            &format!("/{}", T::KIND),
            get(list_texts::<T>).post(create_text::<T>),
        )
        .route(
            &format!("/{}/{{id}}", T::KIND),
            get(fetch_text::<T>)
                .put(replace_text::<T>)
                .delete(remove_text::<T>),
        )
}

async fn list_texts<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Query(query): Query<Search>,
) -> Result<Json<Vec<Entry<T>>>, ApiError> {
    let wanted = query.search.to_lowercase();
    let mut texts = store.texts::<T>((&claims).into()).await?;
    texts.retain(|t| t.text.name().to_lowercase().contains(&wanted));
    Ok(Json(texts))
}

async fn fetch_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<Entry<T>>, ApiError> {
    Ok(Json(store.text::<T>((&claims).into(), id).await?))
}

async fn create_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Json(text): Json<T>,
) -> Result<impl IntoResponse, ApiError> {
    dm_only(&claims)?;
    let entry = store.create_text((&claims).into(), text).await?;
    let location = format!("/{}/{}", T::KIND, entry.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(entry),
    ))
}

async fn replace_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(text): Json<T>,
) -> Result<Json<Entry<T>>, ApiError> {
    dm_only(&claims)?;
    Ok(Json(store.replace_text((&claims).into(), id, text).await?))
}

async fn remove_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    dm_only(&claims)?;
    store.delete_text::<T>((&claims).into(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Search {
    /// Only monsters whose name contains this (any case).
    #[serde(default)]
    search: String,
}

/// The shared monsters of the campaign's edition and its own, by name.
async fn list(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Query(query): Query<Search>,
) -> Result<Json<Vec<MonsterSummary>>, ApiError> {
    let wanted = query.search.to_lowercase();
    let monsters = store.list((&claims).into()).await?;
    Ok(Json(
        monsters
            .iter()
            .filter(|m| m.sheet.name.to_lowercase().contains(&wanted))
            .map(MonsterSummary::from)
            .collect(),
    ))
}

async fn fetch(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<Monster>, ApiError> {
    Ok(Json(store.get((&claims).into(), id).await?))
}

/// Adds a monster to the campaign's own.
async fn create(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Json(sheet): Json<MonsterSheet>,
) -> Result<impl IntoResponse, ApiError> {
    dm_only(&claims)?;
    let monster = store.create((&claims).into(), sheet).await?;
    let location = format!("/monsters/{}", monster.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(monster),
    ))
}

async fn replace(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(sheet): Json<MonsterSheet>,
) -> Result<Json<Monster>, ApiError> {
    dm_only(&claims)?;
    Ok(Json(store.replace((&claims).into(), id, sheet).await?))
}

async fn remove(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    dm_only(&claims)?;
    store.delete((&claims).into(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Players' devices read; only the DM's change anything.
fn dm_only(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_dm() {
        Ok(())
    } else {
        Err(ApiError::DmOnly)
    }
}
