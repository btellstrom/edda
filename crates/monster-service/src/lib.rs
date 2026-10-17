//! Monster service: stat blocks, and the full text of spells and
//! conditions, in a shared library (the SRD, imported by an admin) plus
//! each campaign's own. Every request needs a token from the auth service;
//! a campaign sees the shared ones of its edition and its own, and can
//! change only its own.

/// Errors, as responses.
pub mod error;
/// The HTTP routes.
pub mod routes;
pub mod sqlite;
pub mod store;
pub mod texts;

use std::sync::Arc;

use axum::{Extension, Router};
use edda_auth::server::TokenVerifier;
use edda_server::{Limits, openapi::Conventions};
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

pub use store::MonsterStore;

/// The service's `OpenAPI` description.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        edda_server::health,
        routes::list,
        routes::create,
        routes::fetch,
        routes::replace,
        routes::remove,
        routes::spells::list,
        routes::spells::create,
        routes::spells::fetch,
        routes::spells::replace,
        routes::spells::remove,
        routes::conditions::list,
        routes::conditions::create,
        routes::conditions::fetch,
        routes::conditions::replace,
        routes::conditions::remove,
    ),
    modifiers(&Conventions),
    tags(
        (name = "monsters", description = "Stat blocks: the shared library of the campaign's edition, and the campaign's own. Only DM devices change them."),
        (name = "spells", description = "The text of spells, shared and the campaign's own, like monsters."),
        (name = "conditions", description = "The text of conditions, shared and the campaign's own, like monsters."),
    )
)]
pub struct ApiDoc;

/// Opens (creating if needed) the database and applies migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = edda_server::open_database(database_url).await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Brings the database up to date.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}

/// The service, accepting tokens that `verifier` vouches for, within
/// `limits` (how many monsters, spells and conditions of its own a campaign
/// may keep).
pub fn app(pool: PgPool, verifier: TokenVerifier, limits: Limits) -> Router {
    Router::new()
        .merge(routes::router())
        .with_state(MonsterStore::new(pool))
        .layer(Extension(Arc::new(verifier)))
        .layer(Extension(Arc::new(limits)))
        .layer(TraceLayer::new_for_http())
}
