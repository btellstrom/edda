//! Character service: CRUD over character sheets, in Postgres. Every
//! request needs a token from the auth service, and only sees its own
//! campaign's characters.

/// Errors, as responses.
/// What whoever runs the service asks of it over HTTP.
pub mod admin;
pub mod error;
/// The HTTP routes.
pub mod routes;
/// Characters in Postgres.
pub mod store;

use std::sync::Arc;

use axum::{Extension, Router};
use edda_auth::server::TokenVerifier;
use edda_server::{Limits, openapi::Conventions};
use sqlx::PgPool;

pub use store::CharacterStore;

/// The service's `OpenAPI` description.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        edda_server::health,
        edda_server::ready,
        routes::list,
        routes::create,
        routes::fetch,
        routes::replace,
        routes::remove,
        routes::apply_action,
        routes::set_owners,
    ),
    modifiers(&Conventions),
    tags((name = "characters", description = "Character sheets, and what happens to them in play."))
)]
pub struct ApiDoc;

/// What the handlers share.
#[derive(Clone)]
pub struct AppState {
    pub characters: CharacterStore,
    pub limits: Arc<Limits>,
}

/// Opens (creating if needed) the database and applies migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    edda_server::open_database(database_url, &sqlx::migrate!()).await
}

/// The service, accepting tokens that `verifier` vouches for, within
/// `limits` (how many characters a campaign may keep).
pub fn app(pool: PgPool, verifier: TokenVerifier, limits: Limits) -> Router {
    let ready = edda_server::readiness(pool.clone());
    let state = AppState {
        characters: CharacterStore::new(pool),
        limits: Arc::new(limits),
    };
    Router::new()
        .merge(routes::router())
        .with_state(state)
        .layer(Extension(Arc::new(verifier)))
        .merge(ready)
}
