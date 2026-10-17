use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use chrono::{Duration, Utc};
use edda_auth::{
    PublicKey, Purpose, Role, SignedCert,
    protocol::{
        Campaign, Challenge, ChallengeRequest, Claims, Device, LoginProof, LoginRequest, ServerKey,
        Session, SetupCode, SetupRequest,
    },
    server::{Authenticated, TokenSigner},
};

use edda_server::{Limits, openapi::ErrorBody};

use crate::{error::ApiError, store::Store};

const TOKEN_LIFETIME: Duration = Duration::minutes(15);
const CHALLENGE_LIFETIME: Duration = Duration::minutes(1);
const MAX_NAME_LENGTH: usize = 64;

/// What the handlers share.
pub struct AppState {
    pub store: Store,
    pub signer: TokenSigner,
    pub limits: Limits,
}

type AppStateRef = Arc<AppState>;

/// The routes, signing in limited by `limits`.
pub fn router(limits: &Limits) -> Router<AppStateRef> {
    // What needs no token gets fewer tries.
    let sign_ins = Router::new()
        .route("/setup", post(setup))
        .route("/setup/lookup", post(lookup_setup_code))
        .route("/login/challenge", post(challenge))
        .route("/login", post(login));
    Router::new()
        .route("/health", get(edda_server::health))
        .route("/public-key", get(public_key))
        .merge(limits.guard_sign_ins(sign_ins))
        .route("/campaign", get(campaign))
        .route("/devices", get(devices).post(add_device))
        .route("/devices/{key}", delete(revoke))
}

#[utoipa::path(
    get,
    path = "/public-key",
    tag = "auth",
    security(()),
    responses((status = 200, description = "The key tokens are signed with.", body = ServerKey))
)]
async fn public_key(State(state): State<AppStateRef>) -> Json<ServerKey> {
    Json(ServerKey {
        key: state.signer.public_key(),
    })
}

#[utoipa::path(
    post,
    path = "/setup/lookup",
    tag = "auth",
    security(()),
    request_body = SetupCode,
    responses((status = 200, description = "The campaign it's for.", body = Campaign), (status = 401, description = "Not a valid setup code, or it has expired.", body = ErrorBody))
)]
/// Which campaign a setup code is for, without using it up: the DM's
/// device needs the campaign id to sign its certificate.
async fn lookup_setup_code(
    State(state): State<AppStateRef>,
    Json(request): Json<SetupCode>,
) -> Result<Json<Campaign>, ApiError> {
    let campaign = state
        .store
        .peek_setup_code(&request.code)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid or expired setup code".into()))?;
    Ok(Json(state.store.campaign(campaign).await?))
}

#[utoipa::path(
    post,
    path = "/setup",
    tag = "auth",
    security(()),
    request_body = SetupRequest,
    responses(
        (status = 201, description = "The device is the campaign's DM.", body = Campaign),
        (status = 400, description = "The certificate isn't a DM's, self-signed, for the code's campaign.", body = ErrorBody),
        (status = 401, description = "Not a valid setup code, or it has expired.", body = ErrorBody),
    )
)]
/// Registers a campaign's first (or, after recovery, a new) DM device.
async fn setup(
    State(state): State<AppStateRef>,
    Json(request): Json<SetupRequest>,
) -> Result<(StatusCode, Json<Campaign>), ApiError> {
    let cert = request.cert.verify()?;
    if !request.cert.is_self_signed() || cert.role != Role::Dm {
        return Err(ApiError::BadRequest(
            "setup needs a self-signed DM certificate".into(),
        ));
    }
    check_name(&cert.name)?;
    let mut tx = state.store.pool().begin().await?;
    let campaign = state
        .store
        .take_setup_code(&mut *tx, &request.code)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid or expired setup code".into()))?;
    if campaign != cert.campaign {
        return Err(ApiError::BadRequest(
            "certificate is for a different campaign".into(),
        ));
    }
    state.store.add_device(&mut *tx, &request.cert).await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(state.store.campaign(campaign).await?),
    ))
}

#[utoipa::path(
    post,
    path = "/login/challenge",
    tag = "auth",
    security(()),
    request_body = ChallengeRequest,
    responses(
        (status = 200, description = "What to sign; valid for a minute, once.", body = Challenge),
        (status = 401, description = "Not a device of that campaign.", body = ErrorBody),
    )
)]
async fn challenge(
    State(state): State<AppStateRef>,
    Json(request): Json<ChallengeRequest>,
) -> Result<Json<Challenge>, ApiError> {
    if state
        .store
        .principal(request.campaign, request.key)
        .await?
        .is_none()
    {
        return Err(ApiError::Unauthorized(
            "this key is not registered in the campaign".into(),
        ));
    }
    let challenge = state
        .store
        .create_challenge(
            request.campaign,
            request.key,
            state.signer.public_key(),
            CHALLENGE_LIFETIME,
        )
        .await?;
    Ok(Json(challenge))
}

#[utoipa::path(
    post,
    path = "/login",
    tag = "auth",
    security(()),
    request_body = LoginRequest,
    responses(
        (status = 200, description = "A token, valid for 15 minutes.", body = Session),
        (status = 401, description = "Not a valid signature of a current challenge.", body = ErrorBody),
    )
)]
async fn login(
    State(state): State<AppStateRef>,
    Json(request): Json<LoginRequest>,
) -> Result<Json<Session>, ApiError> {
    let rejected = || ApiError::Unauthorized("invalid or expired challenge".into());
    let issued = state
        .store
        .take_challenge(request.challenge)
        .await?
        .filter(|c| c.expires_at > Utc::now())
        .ok_or_else(rejected)?;
    let proof = LoginProof {
        challenge: request.challenge,
        nonce: issued.nonce,
        campaign: issued.campaign,
        server: state.signer.public_key(),
    };
    issued
        .key
        .verify(Purpose::Login, &proof, &request.signature)?;
    // Checked again: the device may have been revoked since the challenge.
    let (role, name) = state
        .store
        .principal(issued.campaign, issued.key)
        .await?
        .ok_or_else(rejected)?;
    let edition = state.store.campaign(issued.campaign).await?.edition;
    let settings = state.store.settings(issued.campaign).await?;
    let claims = Claims {
        key: issued.key,
        campaign: issued.campaign,
        role,
        name,
        edition,
        settings,
        expires_at: Utc::now() + TOKEN_LIFETIME,
    };
    let token = state.signer.issue(&claims)?;
    Ok(Json(Session { token, claims }))
}

#[utoipa::path(
    get,
    path = "/campaign",
    tag = "auth",
    responses((status = 200, description = "The token's campaign.", body = Campaign))
)]
async fn campaign(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Campaign>, ApiError> {
    Ok(Json(state.store.campaign(claims.campaign).await?))
}

#[utoipa::path(
    get,
    path = "/devices",
    tag = "devices",
    responses((status = 200, description = "Every device of the campaign, revoked ones too, with its certificate.", body = Vec<Device>))
)]
async fn devices(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<Device>>, ApiError> {
    Ok(Json(state.store.devices(claims.campaign).await?))
}

#[utoipa::path(
    post,
    path = "/devices",
    tag = "devices",
    request_body(content = SignedCert, description = "A certificate for the device, signed by this DM device."),
    responses(
        (status = 201, description = "Added.", body = Device),
        (status = 400, description = "Not signed by this device, or not for this campaign.", body = ErrorBody),
        (status = 403, description = "Not a DM device, or the campaign has as many devices as it may.", body = ErrorBody),
        (status = 409, description = "The key is in the campaign already.", body = ErrorBody),
    )
)]
/// A DM device vouches for a new device with a certificate it signed.
async fn add_device(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
    Json(signed): Json<SignedCert>,
) -> Result<(StatusCode, Json<Device>), ApiError> {
    require_dm(&claims)?;
    let cert = signed.verify()?;
    if cert.issuer != claims.key {
        return Err(ApiError::BadRequest(
            "the certificate must be signed by the device adding it".into(),
        ));
    }
    if cert.campaign != claims.campaign {
        return Err(ApiError::BadRequest(
            "certificate is for a different campaign".into(),
        ));
    }
    check_name(&cert.name)?;
    let devices = state.store.devices(claims.campaign).await?;
    let active = devices.iter().filter(|d| d.revoked_at.is_none()).count();
    let active = i64::try_from(active).unwrap_or(i64::MAX);
    let limits = state.limits.with(&claims.settings.quotas);
    limits
        .room_for(active, 1, limits.devices, "devices")
        .map_err(ApiError::Forbidden)?;
    let device = state.store.add_device(state.store.pool(), &signed).await?;
    Ok((StatusCode::CREATED, Json(device)))
}

#[utoipa::path(
    delete,
    path = "/devices/{key}",
    tag = "devices",
    params(("key" = String, Path, description = "The device's key.")),
    responses(
        (status = 200, description = "Revoked: it can't log in any more.", body = Device),
        (status = 400, description = "Not a device key.", body = ErrorBody),
        (status = 403, description = "Not a DM device.", body = ErrorBody),
        (status = 404, description = "No such device in the campaign.", body = ErrorBody),
        (status = 422, description = "The campaign's last DM device.", body = ErrorBody),
    )
)]
async fn revoke(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
    Path(key): Path<String>,
) -> Result<Json<Device>, ApiError> {
    require_dm(&claims)?;
    let key: PublicKey = key
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("bad key: {e}")))?;
    Ok(Json(state.store.revoke(claims.campaign, key).await?))
}

fn require_dm(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_dm() {
        Ok(())
    } else {
        Err(ApiError::Forbidden("only a DM device can do this".into()))
    }
}

fn check_name(name: &str) -> Result<(), ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LENGTH {
        return Err(ApiError::BadRequest(format!(
            "device names must be 1 to {MAX_NAME_LENGTH} characters"
        )));
    }
    Ok(())
}
