//! Combat service: authoritative, event-sourced combat encounters with live
//! updates over WebSocket. Any number of instances can run against the same
//! Postgres database.

pub mod encounters;
pub mod error;
pub mod hub;
pub mod routes;
pub mod services;
pub mod store;
pub mod sync;

use std::sync::Arc;

use axum::{Extension, Router};
use campaign_auth::server::TokenVerifier;
use sqlx::{PgPool, Postgres, migrate::MigrateDatabase, postgres::PgPoolOptions};
use tower_http::trace::TraceLayer;

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The other services to use, as this service itself. `None` uses
    /// none: sheets are left alone, and monsters can't be added by id.
    pub services: Option<services::ServicesConfig>,
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
    let (mut sheet_sync, mut monsters) = (None, None);
    if let Some(config) = config.services {
        let login = Arc::new(services::ServiceLogin::new(&config));
        sheet_sync = config
            .character_service
            .map(|url| sync::SheetSync::new(store.clone(), login.clone(), url));
        monsters = config
            .monster_service
            .map(|url| services::Monsters::new(login, url));
    }
    let hub = hub::Hub::start(store, sheet_sync, monsters).await?;
    Ok(Router::new()
        .merge(routes::router())
        .merge(encounters::router())
        .with_state(hub)
        .layer(Extension(Arc::new(verifier)))
        .layer(TraceLayer::new_for_http()))
}
