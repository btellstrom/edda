//! Character service: CRUD over character sheets, in Postgres. Every
//! request needs a token from the auth service, and only sees its own
//! campaign's characters.

pub mod error;
pub mod routes;
pub mod sqlite;
pub mod store;

use std::sync::Arc;

use axum::{Extension, Router};
use campaign_auth::server::TokenVerifier;
use campaign_server::Limits;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

pub use store::CharacterStore;

#[derive(Clone)]
pub struct AppState {
    pub characters: CharacterStore,
    pub limits: Arc<Limits>,
}

/// Opens (creating if needed) the database and applies migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = campaign_server::open_database(database_url).await?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}

/// The service, accepting tokens that `verifier` vouches for, within
/// `limits` (how many characters a campaign may keep).
pub fn app(pool: PgPool, verifier: TokenVerifier, limits: Limits) -> Router {
    let state = AppState {
        characters: CharacterStore::new(pool),
        limits: Arc::new(limits),
    };
    Router::new()
        .merge(routes::router())
        .with_state(state)
        .layer(Extension(Arc::new(verifier)))
        .layer(TraceLayer::new_for_http())
}
