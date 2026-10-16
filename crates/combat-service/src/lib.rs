//! Combat service: authoritative, event-sourced combat encounters with live
//! updates over WebSocket. Any number of instances can run against the same
//! Postgres database.

pub mod error;
pub mod hub;
pub mod routes;
pub mod store;
pub mod sync;

use std::sync::Arc;

use axum::{Extension, Router};
use campaign_auth::server::TokenVerifier;
use sqlx::{PgPool, Postgres, migrate::MigrateDatabase, postgres::PgPoolOptions};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Where and as whom to write combatants' hit points back to character
    /// sheets. `None` turns the write-back off.
    pub hp_sync: Option<sync::HpSyncConfig>,
}

/// Connects to the database, creating it if needed, and applies migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    if !Postgres::database_exists(database_url).await? {
        // Another instance may be creating it at the same moment.
        if let Err(e) = Postgres::create_database(database_url).await
            && !Postgres::database_exists(database_url).await?
        {
            return Err(e);
        }
    }
    let pool = PgPoolOptions::new().connect(database_url).await?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Safe to run from several instances at once: sqlx serialises migrations
/// with an advisory lock.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}

/// The service, with its background tasks started on the current runtime.
/// Accepts tokens that `verifier` vouches for.
pub async fn app(
    pool: PgPool,
    verifier: TokenVerifier,
    config: Config,
) -> Result<Router, sqlx::Error> {
    let store = store::Store::new(pool);
    let hp_sync = config
        .hp_sync
        .map(|config| sync::HpSync::new(store.clone(), config));
    let hub = hub::Hub::start(store, hp_sync).await?;
    Ok(Router::new()
        .merge(routes::router())
        .with_state(hub)
        .layer(Extension(Arc::new(verifier)))
        // Permissive CORS so browser frontends served from other origins can
        // talk to the API during development. Tighten before deploying.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http()))
}
