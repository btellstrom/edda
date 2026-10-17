use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use campaign_auth::{
    PublicKey, Purpose, Role, SignedCert,
    protocol::{
        Campaign, Challenge, ChallengeRequest, Claims, Device, LoginProof, LoginRequest, ServerKey,
        Session, SetupCode, SetupRequest,
    },
    server::{Authenticated, TokenSigner},
};
use chrono::{Duration, Utc};

use campaign_server::Limits;

use crate::{error::ApiError, store::Store};

const TOKEN_LIFETIME: Duration = Duration::minutes(15);
const CHALLENGE_LIFETIME: Duration = Duration::minutes(1);
const MAX_NAME_LENGTH: usize = 64;

pub struct AppState {
    pub store: Store,
    pub signer: TokenSigner,
    pub limits: Limits,
}

type AppStateRef = Arc<AppState>;

pub fn router(limits: &Limits) -> Router<AppStateRef> {
    // What needs no token gets fewer tries.
    let sign_ins = Router::new()
        .route("/setup", post(setup))
        .route("/setup/lookup", post(lookup_setup_code))
        .route("/login/challenge", post(challenge))
        .route("/login", post(login));
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/public-key", get(public_key))
        .merge(limits.guard_sign_ins(sign_ins))
        .route("/campaign", get(campaign))
        .route("/devices", get(devices).post(add_device))
        .route("/devices/{key}", delete(revoke))
}

async fn public_key(State(state): State<AppStateRef>) -> Json<ServerKey> {
    Json(ServerKey {
        key: state.signer.public_key(),
    })
}

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
    let claims = Claims {
        key: issued.key,
        campaign: issued.campaign,
        role,
        name,
        edition,
        expires_at: Utc::now() + TOKEN_LIFETIME,
    };
    let token = state.signer.issue(&claims)?;
    Ok(Json(Session { token, claims }))
}

async fn campaign(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Campaign>, ApiError> {
    Ok(Json(state.store.campaign(claims.campaign).await?))
}

async fn devices(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<Device>>, ApiError> {
    Ok(Json(state.store.devices(claims.campaign).await?))
}

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
    state
        .limits
        .room_for(active, 1, state.limits.devices, "devices")
        .map_err(ApiError::Forbidden)?;
    let device = state.store.add_device(state.store.pool(), &signed).await?;
    Ok((StatusCode::CREATED, Json(device)))
}

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
