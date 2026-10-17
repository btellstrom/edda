//! The auth service's admin API (see [`edda_server::admin`]): creating a
//! campaign with its DM's setup code, a new setup code, and deleting a
//! campaign with its devices.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, post},
};
use chrono::Duration;
use edda_auth::protocol::{CreatedCampaign, NewCampaign, SetupCode};
use edda_types::admin::Deleted;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{Store, error::ApiError};

/// How long a DM setup code works.
pub const SETUP_CODE_LIFETIME: Duration = Duration::hours(24);

/// The longest campaign name.
const MAX_NAME_LENGTH: usize = 100;

/// The admin API's routes.
pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/admin/campaigns", post(create))
        .route("/admin/campaigns/{id}", delete(remove))
        .route("/admin/campaigns/{id}/setup-code", post(setup_code))
        .with_state(Store::new(pool))
}

/// Creates a campaign, with a code for its DM's first device.
async fn create(
    State(store): State<Store>,
    Json(new): Json<NewCampaign>,
) -> Result<(StatusCode, Json<CreatedCampaign>), ApiError> {
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LENGTH {
        return Err(ApiError::BadRequest(format!(
            "campaign names must be 1 to {MAX_NAME_LENGTH} characters"
        )));
    }
    let campaign = store.create_campaign(name, new.edition).await?;
    let setup_code = store
        .new_setup_code(campaign.id, SETUP_CODE_LIFETIME)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedCampaign {
            campaign,
            setup_code,
        }),
    ))
}

/// A new code for a DM to set up a device with, as when every DM device
/// is lost.
async fn setup_code(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<SetupCode>, ApiError> {
    let code = store.new_setup_code(campaign, SETUP_CODE_LIFETIME).await?;
    Ok(Json(SetupCode { code }))
}

/// Deletes a campaign with its devices; nothing, if it's gone already.
async fn remove(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<Deleted>, ApiError> {
    let deleted = store
        .delete_campaign(campaign)
        .await?
        .map_or(0, |(_, devices)| devices);
    Ok(Json(Deleted { deleted }))
}
