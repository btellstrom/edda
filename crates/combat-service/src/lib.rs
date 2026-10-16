//! Combat service: authoritative, event-sourced combat encounters with live
//! updates over WebSocket.

pub mod error;
pub mod hub;
pub mod routes;
pub mod store;

use std::{str::FromStr, sync::Arc};

use axum::Router;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

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

pub fn app(pool: SqlitePool) -> Router {
    let hub = Arc::new(hub::Hub::new(store::Store::new(pool)));
    Router::new()
        .merge(routes::router())
        .with_state(hub)
        // Permissive CORS so browser frontends served from other origins can
        // talk to the API during development. Tighten before deploying.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
