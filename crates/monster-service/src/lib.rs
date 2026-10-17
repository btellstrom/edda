//! Monster service: stat blocks, and the full text of spells and
//! conditions, in a shared library (the SRD, imported by an admin) plus
//! each campaign's own. Every request needs a token from the auth service;
//! a campaign sees the shared ones of its edition and its own, and can
//! change only its own.

pub mod error;
pub mod routes;
pub mod store;
pub mod texts;

use std::{str::FromStr, sync::Arc};

use axum::{Extension, Router};
use campaign_auth::server::TokenVerifier;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

pub use store::MonsterStore;

/// Opens (creating if needed) the database and applies migrations.
pub async fn connect(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_with(options).await?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}

/// The service, accepting tokens that `verifier` vouches for.
pub fn app(pool: SqlitePool, verifier: TokenVerifier) -> Router {
    Router::new()
        .merge(routes::router())
        .with_state(MonsterStore::new(pool))
        .layer(Extension(Arc::new(verifier)))
        // Permissive CORS so browser frontends served from other origins can
        // talk to the API during development. Tighten before deploying.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
