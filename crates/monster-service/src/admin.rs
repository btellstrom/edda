//! The monster service's admin API (see [`edda_server::admin`]): deleting a
//! campaign's own monsters, spells and conditions.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::delete,
};
use edda_types::admin::Deleted;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{MonsterStore, error::ApiError};

/// The admin API's routes.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/admin/campaigns/{id}", delete(remove))
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
