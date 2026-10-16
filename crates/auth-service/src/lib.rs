//! Auth service: campaigns, the devices admitted to them, and short-lived
//! tokens for the other services. See `campaign-auth` for the cryptography.
//! Several instances can run against the same database.

pub mod error;
pub mod routes;
pub mod store;

use std::sync::Arc;

use axum::{Extension, Router};
use campaign_auth::server::{TokenSigner, TokenVerifier};
use sqlx::{PgPool, Postgres, migrate::MigrateDatabase, postgres::PgPoolOptions};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

pub use store::Store;

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
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

pub async fn app(pool: PgPool) -> Result<Router, error::ApiError> {
    let store = Store::new(pool);
    let signer = TokenSigner::new(&store.token_key().await?);
    let verifier = Arc::new(TokenVerifier::new(signer.public_key()));
    Ok(Router::new()
        .merge(routes::router())
        .with_state(Arc::new(routes::AppState { store, signer }))
        .layer(Extension(verifier))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http()))
}
