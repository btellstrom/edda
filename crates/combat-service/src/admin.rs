//! The combat service's admin API (see [`edda_server::admin`]): deleting a
//! campaign's combats and planned encounters.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::delete,
};
use edda_types::admin::Deleted;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{error::ApiError, store::Store};

/// The admin API's routes.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/admin/campaigns/{id}", delete(remove))
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
