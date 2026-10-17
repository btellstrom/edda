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

use edda_server::openapi::ErrorBody;

use crate::{AppState, error::ApiError};

/// The routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(edda_server::health))
        .route("/characters", get(list).post(create))
        .route("/characters/{id}", get(fetch).put(replace).delete(remove))
        .route("/characters/{id}/actions", post(apply_action))
        .route("/characters/{id}/owners", put(set_owners))
}

/// A `412`: the character as it is now, to re-apply a change to.
#[derive(utoipa::ToSchema)]
pub struct Stale {
    pub error: String,
    pub current: Character,
}

/// Strong `ETag` for a character: its version, quoted per RFC 9110.
pub fn etag(character: &Character) -> String {
    format!("\"{}\"", character.version)
}

/// A character response carrying its `ETag`, so clients can always pick up
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

#[utoipa::path(
    get,
    path = "/characters",
    tag = "characters",
    responses((status = 200, description = "The campaign's characters.", body = Vec<CharacterSummary>))
)]
async fn list(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<CharacterSummary>>, ApiError> {
    let characters = state.characters.list((&claims).into()).await?;
    Ok(Json(
        characters.iter().map(CharacterSummary::from).collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/characters",
    tag = "characters",
    request_body = CharacterSheet,
    responses(
        (status = 201, description = "Created.", body = Character, headers(
            ("Location" = String, description = "Where it is: `/characters/{id}`."),
            ("ETag" = String, description = "Its version, quoted."),
        )),
        (status = 403, description = "The campaign has as many characters as it may keep.", body = ErrorBody),
        (status = 422, description = "Not a valid sheet.", body = ErrorBody),
    )
)]
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

#[utoipa::path(
    get,
    path = "/characters/{id}",
    tag = "characters",
    params(("id" = Uuid, Path, description = "The character's id.")),
    responses(
        (status = 200, description = "The character.", body = Character, headers(("ETag" = String, description = "The character's version, quoted: `\"3\"`."))),
        (status = 404, description = "No such character in the campaign.", body = ErrorBody),
    )
)]
async fn fetch(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Ok(tagged(state.characters.get((&claims).into(), id).await?))
}

#[utoipa::path(
    put,
    path = "/characters/{id}",
    tag = "characters",
    params(
        ("id" = Uuid, Path, description = "The character's id."),
        ("If-Match" = String, Header, description = "The version the new sheet is based on, quoted (`\"3\"`), or `*` to overwrite whatever is there."),
    ),
    request_body = CharacterSheet,
    responses(
        (status = 200, description = "Replaced.", body = Character, headers(("ETag" = String, description = "The character's version, quoted: `\"3\"`."))),
        (status = 400, description = "`If-Match` isn't one version or `*`.", body = ErrorBody),
        (status = 403, description = "Not a character this device plays.", body = ErrorBody),
        (status = 404, description = "No such character in the campaign.", body = ErrorBody),
        (status = 412, description = "Changed since that version: re-apply the change to `current` and try again.", body = Stale),
        (status = 422, description = "Not a valid sheet.", body = ErrorBody),
        (status = 428, description = "No `If-Match`.", body = ErrorBody),
    )
)]
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

#[utoipa::path(
    delete,
    path = "/characters/{id}",
    tag = "characters",
    params(("id" = Uuid, Path, description = "The character's id.")),
    responses(
        (status = 204, description = "Deleted."),
        (status = 403, description = "Not a character this device plays.", body = ErrorBody),
        (status = 404, description = "No such character in the campaign.", body = ErrorBody),
    )
)]
async fn remove(
    State(state): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.characters.delete((&claims).into(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/characters/{id}/actions",
    tag = "characters",
    params(
        ("id" = Uuid, Path, description = "The character's id."),
        ("Idempotency-Key" = Option<String>, Header, description = "Applies the action once per key: sent again, the character comes back unchanged."),
    ),
    request_body = CharacterAction,
    responses(
        (status = 200, description = "Applied.", body = Character, headers(("ETag" = String, description = "The character's version, quoted: `\"3\"`."))),
        (status = 403, description = "Not a character this device plays.", body = ErrorBody),
        (status = 404, description = "No such character in the campaign.", body = ErrorBody),
        (status = 409, description = "No slots or uses left.", body = ErrorBody),
        (status = 422, description = "Not an action this character can take.", body = ErrorBody),
    )
)]
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

#[utoipa::path(
    put,
    path = "/characters/{id}/owners",
    tag = "characters",
    params(("id" = Uuid, Path, description = "The character's id.")),
    request_body(content = Vec<String>, description = "The device keys of the player devices that play it."),
    responses(
        (status = 200, description = "Set.", body = Character, headers(("ETag" = String, description = "The character's version, quoted: `\"3\"`."))),
        (status = 400, description = "Not a device key.", body = ErrorBody),
        (status = 403, description = "Not a DM device.", body = ErrorBody),
        (status = 404, description = "No such character in the campaign.", body = ErrorBody),
    )
)]
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
