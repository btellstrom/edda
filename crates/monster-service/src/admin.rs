//! The monster service's admin API (see [`edda_server::admin`]):
//! exporting, importing and deleting a campaign's own monsters, spells and
//! conditions.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, put},
};
use edda_types::admin::{Deleted, Imported, MonstersExport};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{MonsterStore, error::ApiError};

/// The admin API's routes.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/admin/campaigns/{id}", delete(remove))
        .route("/admin/campaigns/{id}/export", get(export))
        .route("/admin/campaigns/{id}/import", put(import))
        .with_state(MonsterStore::new(pool))
}

/// Deletes a campaign's own monsters, spells and conditions; nothing, if there are none.
async fn remove(
    State(store): State<MonsterStore>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<Deleted>, ApiError> {
    let deleted = store.delete_campaign(campaign).await?;
    Ok(Json(Deleted { deleted }))
}

/// The campaign's own, to keep or to move to another server.
async fn export(
    State(store): State<MonsterStore>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<MonstersExport>, ApiError> {
    Ok(Json(store.export_campaign(campaign).await?))
}

/// Takes in a campaign's own exported from another server; `409` if it
/// has some here.
async fn import(
    State(store): State<MonsterStore>,
    Path(campaign): Path<Uuid>,
    Json(export): Json<MonstersExport>,
) -> Result<(StatusCode, Json<Imported>), ApiError> {
    let imported = store.import_campaign(campaign, &export).await?;
    Ok((StatusCode::CREATED, Json(Imported { imported })))
}
