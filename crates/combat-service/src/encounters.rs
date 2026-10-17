//! Planned encounters: CRUD over each campaign's reusable plans. Plans
//! (and their notes) are the DM's, so player devices can't see them.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use campaign_auth::{protocol::Claims, server::Authenticated};
use campaign_types::encounter::{Encounter, EncounterPlan};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, types::Json as Jsonb};
use uuid::Uuid;

use crate::{error::ApiError, hub::Hub, store};

type Row = (
    Uuid,
    String,
    Jsonb<EncounterPlan>,
    i64,
    DateTime<Utc>,
    DateTime<Utc>,
);

pub fn router() -> Router<std::sync::Arc<Hub>> {
    Router::new()
        .route("/encounters", get(list).post(create))
        .route("/encounters/{id}", get(fetch).put(replace).delete(remove))
}

/// Plans are the DM's: a player's device mustn't learn what's coming.
pub fn dm_only(claims: &Claims) -> Result<(), ApiError> {
    if claims.is_dm() {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "only a DM device can see planned encounters".into(),
        ))
    }
}

/// One of `campaign`'s plans, to start a fight from; a bad request if
/// there's no such plan.
pub async fn plan(hub: &Hub, campaign: Uuid, id: Uuid) -> Result<EncounterPlan, ApiError> {
    match get_one(hub, campaign, id).await {
        Err(ApiError::NotFound) => Err(ApiError::BadRequest(
            "there's no such planned encounter".into(),
        )),
        other => other.map(|e| e.plan),
    }
}

fn check(plan: &EncounterPlan) -> Result<(), ApiError> {
    if plan.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    if plan.monsters.iter().any(|m| m.count == 0) {
        return Err(ApiError::BadRequest(
            "every monster needs a count of at least 1".into(),
        ));
    }
    Ok(())
}

fn from_row(
    (id, campaign, Jsonb(plan), version, created_at, updated_at): Row,
) -> Result<Encounter, ApiError> {
    Ok(Encounter {
        id,
        campaign: campaign
            .parse()
            .map_err(|e: uuid::Error| ApiError::Corrupt(e.to_string()))?,
        plan,
        version: u64::try_from(version).map_err(|e| ApiError::Corrupt(e.to_string()))?,
        created_at,
        updated_at,
    })
}

fn pool(hub: &Hub) -> &PgPool {
    hub.store().pool()
}

/// The campaign's plans, by name.
async fn list(
    State(hub): State<std::sync::Arc<Hub>>,
    Authenticated(claims): Authenticated,
) -> Result<Json<Vec<Encounter>>, ApiError> {
    dm_only(&claims)?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, campaign, plan, version, created_at, updated_at FROM encounters \
         WHERE campaign = $1 ORDER BY lower(name)",
    )
    .bind(claims.campaign.to_string())
    .fetch_all(pool(&hub))
    .await?;
    Ok(Json(
        rows.into_iter().map(from_row).collect::<Result<_, _>>()?,
    ))
}

async fn get_one(hub: &Hub, campaign: Uuid, id: Uuid) -> Result<Encounter, ApiError> {
    let row: Option<Row> = sqlx::query_as(
        "SELECT id, campaign, plan, version, created_at, updated_at FROM encounters \
         WHERE id = $1 AND campaign = $2",
    )
    .bind(id)
    .bind(campaign.to_string())
    .fetch_optional(pool(hub))
    .await?;
    row.map(from_row).transpose()?.ok_or(ApiError::NotFound)
}

async fn fetch(
    State(hub): State<std::sync::Arc<Hub>>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<Encounter>, ApiError> {
    dm_only(&claims)?;
    Ok(Json(get_one(&hub, claims.campaign, id).await?))
}

async fn create(
    State(hub): State<std::sync::Arc<Hub>>,
    Authenticated(claims): Authenticated,
    Json(plan): Json<EncounterPlan>,
) -> Result<impl IntoResponse, ApiError> {
    dm_only(&claims)?;
    check(&plan)?;
    let (kept,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM encounters WHERE campaign = $1")
        .bind(claims.campaign.to_string())
        .fetch_one(pool(&hub))
        .await?;
    let limits = hub.limits();
    limits
        .room_for(kept, 1, limits.plans, "planned encounters")
        .map_err(ApiError::Forbidden)?;
    let now = store::now();
    let encounter = Encounter {
        id: Uuid::new_v4(),
        campaign: claims.campaign,
        plan,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    sqlx::query(
        "INSERT INTO encounters (id, campaign, name, plan, version, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 1, $5, $5)",
    )
    .bind(encounter.id)
    .bind(claims.campaign.to_string())
    .bind(&encounter.plan.name)
    .bind(Jsonb(&encounter.plan))
    .bind(now)
    .execute(pool(&hub))
    .await?;
    let location = format!("/encounters/{}", encounter.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(encounter),
    ))
}

/// Replaces a plan; the last save wins.
async fn replace(
    State(hub): State<std::sync::Arc<Hub>>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(plan): Json<EncounterPlan>,
) -> Result<Json<Encounter>, ApiError> {
    dm_only(&claims)?;
    check(&plan)?;
    let row: Option<Row> = sqlx::query_as(
        "UPDATE encounters SET name = $1, plan = $2, version = version + 1, updated_at = $3 \
         WHERE id = $4 AND campaign = $5 \
         RETURNING id, campaign, plan, version, created_at, updated_at",
    )
    .bind(&plan.name)
    .bind(Jsonb(&plan))
    .bind(store::now())
    .bind(id)
    .bind(claims.campaign.to_string())
    .fetch_optional(pool(&hub))
    .await?;
    Ok(Json(
        row.map(from_row).transpose()?.ok_or(ApiError::NotFound)?,
    ))
}

async fn remove(
    State(hub): State<std::sync::Arc<Hub>>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    dm_only(&claims)?;
    let result = sqlx::query("DELETE FROM encounters WHERE id = $1 AND campaign = $2")
        .bind(id)
        .bind(claims.campaign.to_string())
        .execute(pool(&hub))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
