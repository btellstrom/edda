//! Character service: CRUD over character sheets. Every request needs a
//! token from the auth service, and only sees its own campaign's characters.

pub mod error;
pub mod routes;
pub mod store;

use std::sync::Arc;

use axum::{Extension, Router};
use campaign_auth::server::TokenVerifier;
use campaign_server::Limits;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::str::FromStr;
use tower_http::trace::TraceLayer;

pub use store::CharacterStore;

#[derive(Clone)]
pub struct AppState {
    pub characters: CharacterStore,
    pub limits: Arc<Limits>,
}

/// Opens (creating if needed) the database and applies migrations.
pub async fn connect(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new().connect_with(options).await?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}

/// The service, accepting tokens that `verifier` vouches for, within
/// `limits` (how many characters a campaign may keep).
pub fn app(pool: SqlitePool, verifier: TokenVerifier, limits: Limits) -> Router {
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
