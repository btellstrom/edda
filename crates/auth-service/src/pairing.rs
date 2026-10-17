//! Relaying pairing messages. See `edda_auth::pairing` for the protocol.
//!
//! Messages are kept in Postgres and fetched by long polling, so the two
//! sides may be talking to different instances. The relay enforces what it
//! can without knowing the code: only the DM device that opened a pairing
//! can speak for the host or cancel it, and only one device can ever speak
//! as the joiner, so an attacker gets a single guess per code.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, post, put},
};
use chrono::{DateTime, Utc};
use edda_auth::{
    PublicKey,
    pairing::Side,
    protocol::{Claims, PairingMessage, PairingSlot},
    server::Authenticated,
};
use edda_server::{Limits, openapi::ErrorBody};

use crate::{error::ApiError, routes::AppState};

const PAIRING_LIFETIME: chrono::Duration = chrono::Duration::minutes(10);
/// Once the host has answered, the joiner only needs to collect it.
const AFTER_ANSWER: chrono::Duration = chrono::Duration::minutes(1);
const MAX_SLOT: u16 = 999;
/// Messages per side: the SPAKE2 message, then one encrypted message.
const MESSAGES_PER_SIDE: u8 = 2;
const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const LONG_POLL: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(150);

type AppStateRef = Arc<AppState>;

/// The pairing routes, the message relay limited like signing in.
pub fn router(limits: &Limits) -> Router<AppStateRef> {
    // Joining needs no token, so the messages get fewer tries: someone
    // spraying slots can't spoil everyone's pairings.
    let messages = Router::new()
        .route(
            "/pairings/{slot}/host/{n}",
            put(send_as_host).get(|state, path| receive(state, path, Side::Host)),
        )
        .route(
            "/pairings/{slot}/joiner/{n}",
            put(send_as_joiner).get(|state, path| receive(state, path, Side::Joiner)),
        );
    Router::new()
        .route("/pairings", post(open))
        .route("/pairings/{slot}", delete(cancel))
        .merge(limits.guard_sign_ins(messages))
}

#[utoipa::path(
    post,
    path = "/pairings",
    tag = "pairing",
    responses(
        (status = 201, description = "A pairing, for 10 minutes.", body = PairingSlot),
        (status = 403, description = "Not a DM device.", body = ErrorBody),
        (status = 409, description = "No free pairings; try again shortly.", body = ErrorBody),
    )
)]
async fn open(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
) -> Result<(StatusCode, Json<PairingSlot>), ApiError> {
    require_dm(&claims)?;
    let pool = state.store.pool();
    sqlx::query("DELETE FROM pairings WHERE expires_at < now()")
        .execute(pool)
        .await?;
    let expires_at = Utc::now() + PAIRING_LIFETIME;
    // Random rather than sequential, so a slot number says nothing about
    // other pairings. Retry on the (rare) collision.
    for _ in 0..50 {
        let slot = rand::random_range(1..=MAX_SLOT);
        let inserted = sqlx::query(
            "INSERT INTO pairings (slot, campaign_id, host_key, expires_at) \
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(i32::from(slot))
        .bind(claims.campaign)
        .bind(claims.key.as_bytes().to_vec())
        .bind(expires_at)
        .execute(pool)
        .await?;
        if inserted.rows_affected() == 1 {
            return Ok((StatusCode::CREATED, Json(PairingSlot { slot, expires_at })));
        }
    }
    Err(ApiError::Conflict(
        "no free pairing slots; try again shortly".into(),
    ))
}

#[utoipa::path(
    delete,
    path = "/pairings/{slot}",
    tag = "pairing",
    params(("slot" = u16, Path, description = "The pairing's number: the first part of the code.")),
    responses(
        (status = 204, description = "Cancelled."),
        (status = 404, description = "No such pairing of this device's.", body = ErrorBody),
    )
)]
async fn cancel(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
    Path(slot): Path<u16>,
) -> Result<StatusCode, ApiError> {
    host_of(&state, slot, &claims).await?;
    sqlx::query("DELETE FROM pairings WHERE slot = $1")
        .bind(i32::from(slot))
        .execute(state.store.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/pairings/{slot}/host/{n}",
    tag = "pairing",
    params(("slot" = u16, Path, description = "The pairing's number: the first part of the code."), ("n" = u8, Path, description = "Which message of that side: 0 or 1.")),
    request_body = PairingMessage,
    responses(
        (status = 201, description = "Relayed."),
        (status = 404, description = "No such pairing of this device's.", body = ErrorBody),
        (status = 409, description = "That message was sent already.", body = ErrorBody),
    )
)]
async fn send_as_host(
    State(state): State<AppStateRef>,
    Authenticated(claims): Authenticated,
    Path((slot, n)): Path<(u16, u8)>,
    Json(message): Json<PairingMessage>,
) -> Result<StatusCode, ApiError> {
    host_of(&state, slot, &claims).await?;
    store(&state, slot, Side::Host, n, &message).await?;
    if n == MESSAGES_PER_SIDE - 1 {
        sqlx::query("UPDATE pairings SET expires_at = LEAST(expires_at, $1) WHERE slot = $2")
            .bind(Utc::now() + AFTER_ANSWER)
            .bind(i32::from(slot))
            .execute(state.store.pool())
            .await?;
    }
    Ok(StatusCode::CREATED)
}

#[utoipa::path(
    put,
    path = "/pairings/{slot}/joiner/{n}",
    tag = "pairing",
    security(()),
    params(("slot" = u16, Path, description = "The pairing's number: the first part of the code."), ("n" = u8, Path, description = "Which message of that side: 0 or 1.")),
    request_body = PairingMessage,
    responses(
        (status = 201, description = "Relayed."),
        (status = 404, description = "No such pairing, or it has expired.", body = ErrorBody),
        (status = 409, description = "The code has been used already, or that message was sent already.", body = ErrorBody),
    )
)]
/// Anyone who has the code may join, but only once: the first joiner's
/// first message claims the pairing.
async fn send_as_joiner(
    State(state): State<AppStateRef>,
    Path((slot, n)): Path<(u16, u8)>,
    Json(message): Json<PairingMessage>,
) -> Result<StatusCode, ApiError> {
    active(&state, slot).await?;
    store(&state, slot, Side::Joiner, n, &message).await?;
    Ok(StatusCode::CREATED)
}

/// Waits (up to 20 seconds) for a side's `n`th message: `200` with it, `204`
/// if it hasn't come yet (ask again), `404` once the pairing is gone.
async fn receive(
    State(state): State<AppStateRef>,
    Path((slot, n)): Path<(u16, u8)>,
    side: Side,
) -> Result<Response, ApiError> {
    let started = tokio::time::Instant::now();
    loop {
        active(&state, slot).await?;
        let row: Option<(Vec<u8>,)> = sqlx::query_as(
            "SELECT data FROM pairing_messages WHERE slot = $1 AND side = $2 AND n = $3",
        )
        .bind(i32::from(slot))
        .bind(side.as_str())
        .bind(i16::from(n))
        .fetch_optional(state.store.pool())
        .await?;
        if let Some((data,)) = row {
            return Ok(Json(PairingMessage { data }).into_response());
        }
        if started.elapsed() >= LONG_POLL {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn store(
    state: &AppState,
    slot: u16,
    side: Side,
    n: u8,
    message: &PairingMessage,
) -> Result<(), ApiError> {
    if n >= MESSAGES_PER_SIDE {
        return Err(ApiError::BadRequest("no such message in a pairing".into()));
    }
    if message.data.len() > MAX_MESSAGE_BYTES {
        return Err(ApiError::BadRequest("pairing message too large".into()));
    }
    // A joiner's later message must come from whoever sent its first, but
    // the relay can't tell joiners apart; it doesn't need to: a stranger
    // without the code can't produce anything the host will accept, and
    // the host then cancels the pairing.
    let inserted = sqlx::query(
        "INSERT INTO pairing_messages (slot, side, n, data) VALUES ($1, $2, $3, $4) \
         ON CONFLICT DO NOTHING",
    )
    .bind(i32::from(slot))
    .bind(side.as_str())
    .bind(i16::from(n))
    .bind(&message.data)
    .execute(state.store.pool())
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError::Conflict(match (side, n) {
            (Side::Joiner, 0) => "this code has already been used".into(),
            _ => "that message was already sent".into(),
        }));
    }
    Ok(())
}

/// The pairing's expiry, if it exists and hasn't expired.
async fn active(state: &AppState, slot: u16) -> Result<(PublicKey, DateTime<Utc>), ApiError> {
    let row: Option<(Vec<u8>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT host_key, expires_at FROM pairings WHERE slot = $1 AND expires_at > now()",
    )
    .bind(i32::from(slot))
    .fetch_optional(state.store.pool())
    .await?;
    let (host, expires_at) = row.ok_or(ApiError::NotFound)?;
    let host = host
        .try_into()
        .ok()
        .and_then(|bytes| PublicKey::from_bytes(bytes).ok())
        .ok_or_else(|| ApiError::Internal("stored host key is invalid".into()))?;
    Ok((host, expires_at))
}

/// Checks that the caller is the DM device that opened the pairing.
async fn host_of(state: &AppState, slot: u16, claims: &Claims) -> Result<(), ApiError> {
    let (host, _) = active(state, slot).await?;
    if host != claims.key {
        // Not revealing whether the slot is in use by someone else.
        return Err(ApiError::NotFound);
    }
    Ok(())
}

fn require_dm(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_dm() {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "only a DM device can pair new devices".into(),
        ))
    }
}

/// Receiving, for the OpenAPI description: the routes call [`receive`]
/// with their side, so these are only described, never called. Messages
/// are sealed between the two devices, so reading needs no token.
#[allow(dead_code)]
pub(crate) mod described {
    use edda_auth::protocol::PairingMessage;
    use edda_server::openapi::ErrorBody;

    #[utoipa::path(
        get,
        path = "/pairings/{slot}/host/{n}",
        tag = "pairing",
        security(()),
        params(("slot" = u16, Path, description = "The pairing's number: the first part of the code."), ("n" = u8, Path, description = "Which message of that side: 0 or 1.")),
        responses(
            (status = 200, description = "The host's message.", body = PairingMessage),
            (status = 204, description = "Not sent yet, after waiting up to 20 seconds: ask again."),
            (status = 404, description = "No such pairing, or it has expired.", body = ErrorBody),
        )
    )]
    pub fn receive_from_host() {}

    #[utoipa::path(
        get,
        path = "/pairings/{slot}/joiner/{n}",
        tag = "pairing",
        security(()),
        params(("slot" = u16, Path, description = "The pairing's number: the first part of the code."), ("n" = u8, Path, description = "Which message of that side: 0 or 1.")),
        responses(
            (status = 200, description = "The joiner's message.", body = PairingMessage),
            (status = 204, description = "Not sent yet, after waiting up to 20 seconds: ask again."),
            (status = 404, description = "No such pairing, or it has expired.", body = ErrorBody),
        )
    )]
    pub fn receive_from_joiner() {}
}
