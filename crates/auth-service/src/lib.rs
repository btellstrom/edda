//! Auth service: campaigns, the devices admitted to them, and short-lived
//! tokens for the other services. See `edda-auth` for the cryptography.
//! Several instances can run against the same database.

pub mod error;
pub mod pairing;
pub mod routes;
pub mod store;

use std::sync::Arc;

use axum::{Extension, Router};
use edda_auth::server::{TokenSigner, TokenVerifier};
use edda_server::Limits;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

pub use store::Store;

/// Connects to the database, creating it if needed, and applies migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = edda_server::open_database(database_url).await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

/// The service, within `limits` (its campaigns' devices, and how often
/// clients may try to sign in).
pub async fn app(pool: PgPool, limits: Limits) -> Result<Router, error::ApiError> {
    let store = Store::new(pool);
    let signer = TokenSigner::new(&store.token_key().await?);
    let verifier = Arc::new(TokenVerifier::new(signer.public_key()));
    Ok(Router::new()
        .merge(routes::router(&limits))
        .merge(pairing::router(&limits))
        .with_state(Arc::new(routes::AppState {
            store,
            signer,
            limits,
        }))
        .layer(Extension(verifier))
        .layer(TraceLayer::new_for_http()))
}
