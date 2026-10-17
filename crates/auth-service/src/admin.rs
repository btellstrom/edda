//! The auth service's admin API (see [`edda_server::admin`]): creating a
//! campaign with its DM's setup code, a new setup code, what a campaign
//! may keep and do, exporting and importing a campaign with its devices,
//! and deleting one.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
};
use chrono::Duration;
use edda_auth::protocol::{AuthExport, CampaignSettings, CreatedCampaign, NewCampaign, SetupCode};
use edda_types::admin::{Deleted, Imported};
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
        .route(
            "/admin/campaigns/{id}/settings",
            get(settings).put(set_settings),
        )
        .route("/admin/campaigns/{id}/export", get(export))
        .route("/admin/campaigns/{id}/import", put(import))
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

/// The campaign and its devices, to keep or to move to another server.
async fn export(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<AuthExport>, ApiError> {
    store
        .export_campaign(campaign)
        .await?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// Takes in a campaign exported from another server; `409` if it's here.
async fn import(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
    Json(export): Json<AuthExport>,
) -> Result<(StatusCode, Json<Imported>), ApiError> {
    if export.campaign.id != campaign {
        return Err(ApiError::BadRequest(
            "that's another campaign's export".into(),
        ));
    }
    let imported = store.import_campaign(&export).await?;
    Ok((StatusCode::CREATED, Json(Imported { imported })))
}

/// What the campaign may keep and do.
async fn settings(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
) -> Result<Json<CampaignSettings>, ApiError> {
    store.campaign(campaign).await?;
    Ok(Json(store.settings(campaign).await?))
}

/// Sets what the campaign may keep and do (read-only, or limits of its
/// own); its devices' tokens carry it from their next login, within 15
/// minutes.
async fn set_settings(
    State(store): State<Store>,
    Path(campaign): Path<Uuid>,
    Json(settings): Json<CampaignSettings>,
) -> Result<Json<CampaignSettings>, ApiError> {
    store.set_settings(campaign, &settings).await?;
    Ok(Json(settings))
}
