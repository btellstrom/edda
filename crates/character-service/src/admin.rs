//! The character service's admin API (see [`edda_server::admin`]):
//! exporting, importing and deleting a campaign's characters.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, put},
};
use edda_types::admin::{CharactersExport, Deleted, Imported};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{CharacterStore, error::ApiError};

/// The admin API's routes.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/admin/campaigns/{id}", delete(remove))
        .route("/admin/campaigns/{id}/export", get(export))
        .route("/admin/campaigns/{id}/import", put(import))
        .with_state(CharacterStore::new(pool))
}

/// Deletes a campaign's characters; nothing, if there are none.
async fn remove(
    State(store): State<CharacterStore>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<Deleted>, ApiError> {
    let deleted = store.delete_campaign(campaign).await?;
    Ok(Json(Deleted { deleted }))
}

/// The campaign's characters, to keep or to move to another server.
async fn export(
    State(store): State<CharacterStore>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<CharactersExport>, ApiError> {
    let characters = store.export_campaign(campaign).await?;
    Ok(Json(CharactersExport { characters }))
}

/// Takes in characters exported from another server; `409` if the
/// campaign has some here.
async fn import(
    State(store): State<CharacterStore>,
    Path(campaign): Path<Uuid>,
    Json(export): Json<CharactersExport>,
) -> Result<(StatusCode, Json<Imported>), ApiError> {
    let imported = store.import_campaign(campaign, &export.characters).await?;
    Ok((StatusCode::CREATED, Json(Imported { imported })))
}
