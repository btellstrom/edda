use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use edda_auth::{protocol::Claims, server::Authenticated};
use edda_server::Limits;
use edda_types::{
    monster::{Monster, MonsterSheet, MonsterSummary},
    rules_text::{ConditionText, Entry, RulesText, SpellText},
};
use serde::Deserialize;
use uuid::Uuid;

use edda_server::openapi::ErrorBody;

use crate::{MonsterStore, error::ApiError};

/// The routes.
pub fn router() -> Router<MonsterStore> {
    Router::new()
        .route("/health", get(edda_server::health))
        .route("/monsters", get(list).post(create))
        .route("/monsters/{id}", get(fetch).put(replace).delete(remove))
        .merge(texts::<SpellText>())
        .merge(texts::<ConditionText>())
}

/// `/spells` or `/conditions`, like `/monsters` but whole entries in the
/// list (they're short).
fn texts<T: RulesText>() -> Router<MonsterStore> {
    Router::new()
        .route(
            &format!("/{}", T::KIND),
            get(list_texts::<T>).post(create_text::<T>),
        )
        .route(
            &format!("/{}/{{id}}", T::KIND),
            get(fetch_text::<T>)
                .put(replace_text::<T>)
                .delete(remove_text::<T>),
        )
}

async fn list_texts<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Query(query): Query<Search>,
) -> Result<Json<Vec<Entry<T>>>, ApiError> {
    let wanted = query.search.to_lowercase();
    let mut texts = store.texts::<T>((&claims).into()).await?;
    texts.retain(|t| t.text.name().to_lowercase().contains(&wanted));
    Ok(Json(texts))
}

async fn fetch_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<Entry<T>>, ApiError> {
    Ok(Json(store.text::<T>((&claims).into(), id).await?))
}

async fn create_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Extension(limits): Extension<Arc<Limits>>,
    Json(text): Json<T>,
) -> Result<impl IntoResponse, ApiError> {
    dm_only(&claims)?;
    let kept = store.count_own_texts::<T>(claims.campaign).await?;
    limits
        .room_for(kept, 1, limits.rules_texts, T::KIND)
        .map_err(ApiError::Limit)?;
    let entry = store.create_text((&claims).into(), text).await?;
    let location = format!("/{}/{}", T::KIND, entry.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(entry),
    ))
}

async fn replace_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(text): Json<T>,
) -> Result<Json<Entry<T>>, ApiError> {
    dm_only(&claims)?;
    Ok(Json(store.replace_text((&claims).into(), id, text).await?))
}

async fn remove_text<T: RulesText>(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    dm_only(&claims)?;
    store.delete_text::<T>((&claims).into(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, utoipa::IntoParams)]
pub(crate) struct Search {
    /// Only those whose name contains this (any case).
    #[serde(default)]
    search: String,
}

#[utoipa::path(
    get,
    path = "/monsters",
    tag = "monsters",
    params(Search),
    responses((status = 200, description = "The monsters, by name.", body = Vec<MonsterSummary>))
)]
/// The shared monsters of the campaign's edition and its own, by name.
async fn list(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Query(query): Query<Search>,
) -> Result<Json<Vec<MonsterSummary>>, ApiError> {
    let wanted = query.search.to_lowercase();
    let monsters = store.list((&claims).into()).await?;
    Ok(Json(
        monsters
            .iter()
            .filter(|m| m.sheet.name.to_lowercase().contains(&wanted))
            .map(MonsterSummary::from)
            .collect(),
    ))
}

#[utoipa::path(
    get,
    path = "/monsters/{id}",
    tag = "monsters",
    params(("id" = Uuid, Path, description = "The monster's id.")),
    responses((status = 200, description = "The stat block.", body = Monster), (status = 404, description = "No such monster, shared or the campaign's.", body = ErrorBody))
)]
async fn fetch(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<Monster>, ApiError> {
    Ok(Json(store.get((&claims).into(), id).await?))
}

#[utoipa::path(
    post,
    path = "/monsters",
    tag = "monsters",
    request_body = MonsterSheet,
    responses(
        (status = 201, description = "Added.", body = Monster, headers(("Location" = String, description = "Where it is: `/monsters/{id}`."))),
        (status = 403, description = "Not a DM device, or the campaign has as many monsters as it may keep.", body = ErrorBody),
        (status = 422, description = "Not a valid stat block.", body = ErrorBody),
    )
)]
/// Adds a monster to the campaign's own.
async fn create(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Extension(limits): Extension<Arc<Limits>>,
    Json(sheet): Json<MonsterSheet>,
) -> Result<impl IntoResponse, ApiError> {
    dm_only(&claims)?;
    let kept = store.count_own(claims.campaign).await?;
    limits
        .room_for(kept, 1, limits.monsters, "monsters")
        .map_err(ApiError::Limit)?;
    let monster = store.create((&claims).into(), sheet).await?;
    let location = format!("/monsters/{}", monster.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(monster),
    ))
}

/// Replaces one of the campaign's own.
#[utoipa::path(
    put,
    path = "/monsters/{id}",
    tag = "monsters",
    params(("id" = Uuid, Path, description = "The monster's id.")),
    request_body = MonsterSheet,
    responses(
        (status = 200, description = "Replaced.", body = Monster),
        (status = 403, description = "Not a DM device, or a shared monster.", body = ErrorBody),
        (status = 404, description = "No such monster, shared or the campaign's.", body = ErrorBody),
        (status = 422, description = "Not a valid stat block.", body = ErrorBody),
    )
)]
async fn replace(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(sheet): Json<MonsterSheet>,
) -> Result<Json<Monster>, ApiError> {
    dm_only(&claims)?;
    Ok(Json(store.replace((&claims).into(), id, sheet).await?))
}

/// Deletes one of the campaign's own.
#[utoipa::path(
    delete,
    path = "/monsters/{id}",
    tag = "monsters",
    params(("id" = Uuid, Path, description = "The monster's id.")),
    responses(
        (status = 204, description = "Deleted."),
        (status = 403, description = "Not a DM device, or a shared monster.", body = ErrorBody),
        (status = 404, description = "No such monster, shared or the campaign's.", body = ErrorBody),
    )
)]
async fn remove(
    State(store): State<MonsterStore>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    dm_only(&claims)?;
    store.delete((&claims).into(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Players' devices read; only the DM's change anything.
fn dm_only(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_dm() {
        Ok(())
    } else {
        Err(ApiError::DmOnly)
    }
}

/// `/spells` for the `OpenAPI` description, as the handlers are generic:
/// these are only described, never called.
#[allow(dead_code)]
pub(crate) mod spells {
    use super::Search;
    use edda_server::openapi::ErrorBody;
    use edda_types::rules_text::{Entry, SpellText};

    #[utoipa::path(
        get,
        path = "/spells",
        tag = "spells",
        params(Search),
        responses((status = 200, description = "The shared ones of the campaign's edition and its own, by name.", body = [Entry<SpellText>]))
    )]
    pub fn list() {}

    #[utoipa::path(
        get,
        path = "/spells/{id}",
        tag = "spells",
        params(("id" = Uuid, Path)),
        responses(
            (status = 200, description = "One, shared or the campaign's own.", body = Entry<SpellText>),
            (status = 404, description = "No such one.", body = ErrorBody),
        )
    )]
    pub fn fetch() {}

    #[utoipa::path(
        post,
        path = "/spells",
        tag = "spells",
        request_body = SpellText,
        responses(
            (status = 201, description = "Added to the campaign's own.", body = Entry<SpellText>, headers(("Location" = String))),
            (status = 403, description = "Not a DM device, or the campaign has as many as it may keep.", body = ErrorBody),
            (status = 422, description = "Not valid.", body = ErrorBody),
        )
    )]
    pub fn create() {}

    #[utoipa::path(
        put,
        path = "/spells/{id}",
        tag = "spells",
        params(("id" = Uuid, Path)),
        request_body = SpellText,
        responses(
            (status = 200, description = "Replaced.", body = Entry<SpellText>),
            (status = 403, description = "Not a DM device, or a shared one.", body = ErrorBody),
            (status = 404, description = "No such one.", body = ErrorBody),
            (status = 422, description = "Not valid.", body = ErrorBody),
        )
    )]
    pub fn replace() {}

    #[utoipa::path(
        delete,
        path = "/spells/{id}",
        tag = "spells",
        params(("id" = Uuid, Path)),
        responses(
            (status = 204, description = "Deleted."),
            (status = 403, description = "Not a DM device, or a shared one.", body = ErrorBody),
            (status = 404, description = "No such one.", body = ErrorBody),
        )
    )]
    pub fn remove() {}
}

/// `/conditions` for the `OpenAPI` description, as the handlers are generic:
/// these are only described, never called.
#[allow(dead_code)]
pub(crate) mod conditions {
    use super::Search;
    use edda_server::openapi::ErrorBody;
    use edda_types::rules_text::{ConditionText, Entry};

    #[utoipa::path(
        get,
        path = "/conditions",
        tag = "conditions",
        params(Search),
        responses((status = 200, description = "The shared ones of the campaign's edition and its own, by name.", body = [Entry<ConditionText>]))
    )]
    pub fn list() {}

    #[utoipa::path(
        get,
        path = "/conditions/{id}",
        tag = "conditions",
        params(("id" = Uuid, Path)),
        responses(
            (status = 200, description = "One, shared or the campaign's own.", body = Entry<ConditionText>),
            (status = 404, description = "No such one.", body = ErrorBody),
        )
    )]
    pub fn fetch() {}

    #[utoipa::path(
        post,
        path = "/conditions",
        tag = "conditions",
        request_body = ConditionText,
        responses(
            (status = 201, description = "Added to the campaign's own.", body = Entry<ConditionText>, headers(("Location" = String))),
            (status = 403, description = "Not a DM device, or the campaign has as many as it may keep.", body = ErrorBody),
            (status = 422, description = "Not valid.", body = ErrorBody),
        )
    )]
    pub fn create() {}

    #[utoipa::path(
        put,
        path = "/conditions/{id}",
        tag = "conditions",
        params(("id" = Uuid, Path)),
        request_body = ConditionText,
        responses(
            (status = 200, description = "Replaced.", body = Entry<ConditionText>),
            (status = 403, description = "Not a DM device, or a shared one.", body = ErrorBody),
            (status = 404, description = "No such one.", body = ErrorBody),
            (status = 422, description = "Not valid.", body = ErrorBody),
        )
    )]
    pub fn replace() {}

    #[utoipa::path(
        delete,
        path = "/conditions/{id}",
        tag = "conditions",
        params(("id" = Uuid, Path)),
        responses(
            (status = 204, description = "Deleted."),
            (status = 403, description = "Not a DM device, or a shared one.", body = ErrorBody),
            (status = 404, description = "No such one.", body = ErrorBody),
        )
    )]
    pub fn remove() {}
}
