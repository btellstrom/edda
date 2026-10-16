//! Character service: CRUD over character sheets.

pub mod error;
pub mod routes;
pub mod store;

use axum::Router;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::str::FromStr;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

pub use store::CharacterStore;

#[derive(Clone)]
pub struct AppState {
    pub characters: CharacterStore,
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

pub fn app(pool: SqlitePool) -> Router {
    let state = AppState {
        characters: CharacterStore::new(pool),
    };
    Router::new()
        .merge(routes::router())
        .with_state(state)
        // Permissive CORS so browser frontends served from other origins can
        // talk to the API during development. Tighten before deploying.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
