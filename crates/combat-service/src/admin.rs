//! The combat service's admin API (see [`edda_server::admin`]):
//! exporting, importing and deleting a campaign's combats and planned
//! encounters.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, put},
};
use edda_types::admin::{CombatExport, Deleted, Imported};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{error::ApiError, store::Store};

/// The admin API's routes.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/admin/campaigns/{id}", delete(remove))
        .route("/admin/campaigns/{id}/export", get(export))
        .route("/admin/campaigns/{id}/import", put(import))
        .with_state(Store::new(pool))
}

/// Deletes a campaign's combats and planned encounters; nothing, if there are none.
async fn remove(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<Deleted>, ApiError> {
    let deleted = store.delete_campaign(&campaign.to_string()).await?;
    Ok(Json(Deleted { deleted }))
}

/// The campaign's combats and plans, to keep or to move to another server.
async fn export(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<CombatExport>, ApiError> {
    Ok(Json(store.export_campaign(campaign).await?))
}

/// Takes in combats and plans exported from another server; `409` if the
/// campaign has some here.
async fn import(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
    Json(export): Json<CombatExport>,
) -> Result<(StatusCode, Json<Imported>), ApiError> {
    let imported = store.import_campaign(campaign, &export).await?;
    Ok((StatusCode::CREATED, Json(Imported { imported })))
}
