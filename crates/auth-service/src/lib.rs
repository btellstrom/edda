//! Auth service: campaigns, the devices admitted to them, and short-lived
//! tokens for the other services. See `edda-auth` for the cryptography.
//! Several instances can run against the same database.

/// Errors, as responses.
pub mod error;
pub mod pairing;
/// Setting up campaigns, logging in, and devices.
pub mod routes;
pub mod store;

use std::sync::Arc;

use axum::{Extension, Router};
use edda_auth::server::{TokenSigner, TokenVerifier};
use edda_server::{Limits, openapi::Conventions};
use sqlx::PgPool;

pub use store::Store;

/// The service's `OpenAPI` description.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        edda_server::health,
        edda_server::ready,
        routes::public_key,
        routes::lookup_setup_code,
        routes::setup,
        routes::challenge,
        routes::login,
        routes::campaign,
        routes::devices,
        routes::add_device,
        routes::revoke,
        pairing::open,
        pairing::cancel,
        pairing::send_as_host,
        pairing::described::receive_from_host,
        pairing::send_as_joiner,
        pairing::described::receive_from_joiner,
    ),
    modifiers(&Conventions),
    tags(
        (name = "auth", description = "Setting up a campaign's first DM device, and logging in."),
        (name = "devices", description = "A campaign's devices: DM devices add and revoke them."),
        (name = "pairing", description = "Relaying the messages that pair a new device with a DM's, by a code read out between them."),
    )
)]
pub struct ApiDoc;

/// Connects to the database, creating it if needed, and applies migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    edda_server::open_database(database_url, &sqlx::migrate!()).await
}

/// The service, within `limits` (its campaigns' devices, and how often
/// clients may try to sign in).
pub async fn app(pool: PgPool, limits: Limits) -> Result<Router, error::ApiError> {
    let ready = edda_server::readiness(pool.clone());
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
        .merge(ready))
}
