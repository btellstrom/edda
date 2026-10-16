//! Combat service: authoritative, event-sourced combat encounters with live
//! updates over WebSocket. Any number of instances can run against the same
//! Postgres database.

pub mod error;
pub mod hub;
pub mod routes;
pub mod store;
pub mod sync;

use axum::Router;
use sqlx::{PgPool, Postgres, migrate::MigrateDatabase, postgres::PgPoolOptions};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Where to write combatants' hit points back to. `None` turns the
    /// write-back off.
    pub character_service: Option<String>,
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
pub async fn app(pool: PgPool, config: Config) -> Result<Router, sqlx::Error> {
    let store = store::Store::new(pool);
    let hp_sync = config
        .character_service
        .map(|url| sync::HpSync::new(store.clone(), campaign_client::Client::new(url)));
    let hub = hub::Hub::start(store, hp_sync).await?;
    Ok(Router::new()
        .merge(routes::router())
        .with_state(hub)
        // Permissive CORS so browser frontends served from other origins can
        // talk to the API during development. Tighten before deploying.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http()))
}
