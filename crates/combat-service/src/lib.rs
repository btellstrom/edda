//! Combat service: authoritative, event-sourced combat encounters with live
//! updates over WebSocket. Any number of instances can run against the same
//! Postgres database.

pub mod encounters;
/// Errors, as responses.
pub mod error;
pub mod hub;
/// The `/combats` routes and the WebSocket.
pub mod routes;
pub mod services;
pub mod store;
pub mod sync;

use std::sync::Arc;

use axum::{Extension, Router};
use edda_auth::server::TokenVerifier;
use edda_server::{Limits, openapi::Conventions};
use sqlx::{PgPool, postgres::PgListener};

/// The service's `OpenAPI` description.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        edda_server::health,
        edda_server::ready,
        routes::list,
        routes::create,
        routes::snapshot,
        routes::log,
        routes::command,
        routes::websocket,
        encounters::list,
        encounters::create,
        encounters::fetch,
        encounters::replace,
        encounters::remove,
    ),
    components(schemas(
        edda_types::combat::ClientMessage,
        edda_types::combat::ServerMessage,
    )),
    modifiers(&Conventions),
    tags(
        (name = "combats", description = "Live, event-sourced combats."),
        (name = "encounters", description = "Planned encounters: the DM's only."),
    )
)]
pub struct ApiDoc;

/// Where the other services are, the limits, and where to listen for
/// other instances' events.
#[derive(Clone, Default)]
pub struct Config {
    /// The other services to use, as this service itself. `None` uses
    /// none: sheets are left alone, and monsters can't be added by id.
    pub services: Option<services::ServicesConfig>,
    /// How much a campaign may keep, and how many may watch a combat.
    pub limits: Limits,
    /// The database to `LISTEN` on, by a connection of its own (see
    /// [`edda_server::direct_database_url`]); `None` takes one from the
    /// pool, which works unless the pool goes through `PgBouncer`.
    pub listen_url: Option<String>,
}

/// Leaves the URL out: it holds the database's password.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("services", &self.services)
            .field("limits", &self.limits)
            .field("listens_directly", &self.listen_url.is_some())
            .finish()
    }
}

/// Connects to the database, creating it if needed, and applies migrations.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    edda_server::open_database(database_url, &sqlx::migrate!()).await
}

/// The service, with its background tasks started on the current runtime.
/// Accepts tokens that `verifier` vouches for.
pub async fn app(
    pool: PgPool,
    verifier: TokenVerifier,
    config: Config,
) -> Result<Router, sqlx::Error> {
    let ready = edda_server::readiness(pool.clone());
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
    let listener = match &config.listen_url {
        Some(url) => PgListener::connect(url).await?,
        None => PgListener::connect_with(store.pool()).await?,
    };
    let hub = hub::Hub::start(store, listener, sheet_sync, monsters, config.limits).await?;
    Ok(Router::new()
        .merge(routes::router())
        .merge(encounters::router())
        .with_state(hub)
        .layer(Extension(Arc::new(verifier)))
        .merge(ready))
}
