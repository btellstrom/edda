use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use campaign_auth::server::Authenticated;
use campaign_types::monster::{Monster, MonsterSheet, MonsterSummary};
use serde::Deserialize;
use uuid::Uuid;

use crate::{MonsterStore, error::ApiError};

pub fn router() -> Router<MonsterStore> {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/monsters", get(list).post(create))
        .route("/monsters/{id}", get(fetch).put(replace).delete(remove))
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
    Ok(Json(store.replace((&claims).into(), id, sheet).await?))
}

async fn remove(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    store.delete((&claims).into(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}
