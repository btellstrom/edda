use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, post, put},
};
use edda_auth::{PublicKey, server::Authenticated};
use edda_types::{Character, CharacterAction, CharacterSheet, CharacterSummary};
use uuid::Uuid;

use crate::{AppState, error::ApiError};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/characters", get(list).post(create))
        .route("/characters/{id}", get(fetch).put(replace).delete(remove))
        .route("/characters/{id}/actions", post(apply_action))
        .route("/characters/{id}/owners", put(set_owners))
}

/// Strong ETag for a character: its version, quoted per RFC 9110.
pub fn etag(character: &Character) -> String {
    format!("\"{}\"", character.version)
}

/// A character response carrying its ETag, so clients can always pick up
/// the version from the header as well as the body.
fn tagged(character: Character) -> impl IntoResponse {
    ([(header::ETAG, etag(&character))], Json(character))
}

/// Parses `If-Match` into the version the client expects, or `None` for `*`
/// (overwrite whatever is there).
fn expected_version(headers: &HeaderMap) -> Result<Option<u64>, ApiError> {
    let value = headers
        .get(header::IF_MATCH)
        .ok_or(ApiError::PreconditionRequired)?
        .to_str()
        .map_err(|_| ApiError::BadRequest("If-Match is not valid text".into()))?
        .trim();
    if value == "*" {
        return Ok(None);
    }
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .and_then(|v| v.parse().ok())
        .map(Some)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "If-Match must be a single ETag like \"3\" or *, got {value}"
            ))
        })
}

async fn list(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<CharacterSummary>>, ApiError> {
    let characters = state.characters.list((&claims).into()).await?;
    Ok(Json(
        characters.iter().map(CharacterSummary::from).collect(),
    ))
}

async fn create(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Json(sheet): Json<CharacterSheet>,
) -> Result<impl IntoResponse, ApiError> {
    let kept = state.characters.count(claims.campaign).await?;
    state
        .limits
        .room_for(kept, 1, state.limits.characters, "characters")
        .map_err(ApiError::Forbidden)?;
    let character = state.characters.create((&claims).into(), sheet).await?;
    let location = format!("/characters/{}", character.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        tagged(character),
    ))
}

async fn fetch(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(tagged(state.characters.get((&claims).into(), id).await?))
}

async fn replace(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(sheet): Json<CharacterSheet>,
) -> Result<impl IntoResponse, ApiError> {
    let expected = expected_version(&headers)?;
    let saved = state
        .characters
        .replace((&claims).into(), id, expected, sheet)
        .await?;
    Ok(tagged(saved))
}

async fn remove(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.characters.delete((&claims).into(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Applies an action; with an `Idempotency-Key` header, once per key.
async fn apply_action(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(action): Json<CharacterAction>,
) -> Result<impl IntoResponse, ApiError> {
    let key = headers
        .get("idempotency-key")
        .map(|v| {
            v.to_str()
                .map_err(|_| ApiError::BadRequest("Idempotency-Key is not valid text".into()))
        })
        .transpose()?;
    Ok(tagged(
        state
            .characters
            .apply((&claims).into(), id, &action, key)
            .await?,
    ))
}

/// Sets which player devices play a character (DM devices only): a list of
/// device keys, as `auth devices` shows them.
async fn set_owners(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(owners): Json<Vec<String>>,
) -> Result<impl IntoResponse, ApiError> {
    if !claims.is_dm() {
        return Err(ApiError::Forbidden(
            "only a DM device chooses who plays a character".into(),
        ));
    }
    for key in &owners {
        key.parse::<PublicKey>()
            .map_err(|_| ApiError::BadRequest(format!("{key} isn't a device key")))?;
    }
    Ok(tagged(
        state
            .characters
            .set_owners((&claims).into(), id, &owners)
            .await?,
    ))
}
