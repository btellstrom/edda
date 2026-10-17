//! What every edda service shares: logging, opening its
//! database, finding the auth service's key, serving (with the CORS setting
//! and the limits), error responses, and the conventions of their `OpenAPI`
//! descriptions.

pub mod admin;
pub mod limits;
pub mod measure;
pub mod observe;
pub mod openapi;
#[cfg(feature = "testing")]
pub mod testing;

pub use limits::Limits;

use std::{error::Error, fmt::Display, time::Duration};

use axum::{
    Json, Router,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use edda_auth::PublicKey;
use edda_client::auth::AuthClient;
use serde_json::json;
use sqlx::{
    Connection, PgConnection, PgPool, Postgres,
    migrate::{MigrateDatabase, Migrator},
    postgres::PgPoolOptions,
};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing_subscriber::EnvFilter;

/// `GET /health`: answers `ok` while the service runs.
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    security(()),
    responses((status = 200, description = "The service is running.", body = String, example = "ok"))
)]
#[expect(clippy::unused_async, reason = "axum handlers are async")]
pub async fn health() -> &'static str {
    "ok"
}

/// How long `/ready` waits for the database before saying it isn't.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// `GET /ready`, for whatever runs the service (Kubernetes, compose): `200`
/// while it can reach its database, `503` while it can't, so it gets no
/// requests until it can again. `/health` only says it's running.
pub fn readiness(pool: PgPool) -> Router {
    Router::new()
        .route("/ready", axum::routing::get(ready))
        .with_state(pool)
}

/// `GET /ready`: see [`readiness`].
#[utoipa::path(
    get,
    path = "/ready",
    tag = "health",
    security(()),
    responses(
        (status = 200, description = "The service can reach its database.", body = String, example = "ready"),
        (status = 503, description = "It can't, for now.", body = openapi::ErrorBody),
    )
)]
pub async fn ready(axum::extract::State(pool): axum::extract::State<PgPool>) -> Response {
    let checked = tokio::time::timeout(READY_TIMEOUT, sqlx::query("SELECT 1").execute(&pool)).await;
    match checked {
        Ok(Ok(_)) => "ready".into_response(),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "not ready: the database refuses");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &"the database can't be reached",
            )
        }
        Err(_) => {
            tracing::warn!("not ready: the database doesn't answer");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &"the database can't be reached",
            )
        }
    }
}

/// Logs to stderr: `crate_name`'s own messages, this crate's (serving,
/// internal errors) and each request, at info, unless `RUST_LOG` says
/// otherwise. `LOG_FORMAT=json` writes a JSON object a line, for a log
/// collector, and `OTEL_EXPORTER_OTLP_ENDPOINT` sends traces to an
/// OpenTelemetry collector; see [`observe`].
pub fn init_logging(crate_name: &str) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| format!("{crate_name}=info,edda_server=info,tower_http=info").into());
    let json = std::env::var("LOG_FORMAT").is_ok_and(|f| f == "json");
    let traces = observe::traces(crate_name).unwrap_or_else(|e| {
        eprintln!("not sending traces: {e}");
        None
    });
    tracing::subscriber::set_global_default(observe::subscriber(
        json,
        filter,
        std::io::stderr,
        traces,
    ))
    .expect("logging is set up once");
}

/// A connection pool to the service's Postgres database at `url`, after
/// creating the database if there's none yet and bringing it up to date
/// with `migrations`. Those two go through [`direct_database_url`], as
/// they need a session of their own (sqlx serialises migrations with an
/// advisory lock, so instances may start at once).
pub async fn open_database(url: &str, migrations: &Migrator) -> Result<PgPool, sqlx::Error> {
    let direct = direct_database_url(url);
    if !Postgres::database_exists(&direct).await? {
        // Another instance may be creating it at the same moment.
        if let Err(e) = Postgres::create_database(&direct).await
            && !Postgres::database_exists(&direct).await?
        {
            return Err(e);
        }
    }
    let mut session = PgConnection::connect(&direct).await?;
    migrations.run(&mut session).await?;
    session.close().await?;
    PgPoolOptions::new().connect(url).await
}

/// Where to reach the database for what needs a session of its own:
/// migrations, and the combat service's `LISTEN`. `DATABASE_DIRECT_URL`
/// when `url` goes through a pooler that hands out connections per
/// transaction (`PgBouncer`'s transaction mode), otherwise `url` itself.
pub fn direct_database_url(url: &str) -> String {
    direct_or(url, std::env::var("DATABASE_DIRECT_URL").ok())
}

fn direct_or(url: &str, direct: Option<String>) -> String {
    direct
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| url.to_owned())
}

/// The key the auth service signs tokens with: `AUTH_PUBLIC_KEY` if set,
/// otherwise fetched from the auth service at `auth_service` (waiting a
/// while for it to start). Set the key explicitly unless the network
/// between the services is trusted.
pub async fn auth_key(auth_service: &str) -> Result<PublicKey, Box<dyn Error>> {
    if let Ok(key) = std::env::var("AUTH_PUBLIC_KEY") {
        return Ok(key.parse()?);
    }
    let key = AuthClient::new(auth_service)
        .wait_for_server_key(Duration::from_secs(30))
        .await
        .map_err(|e| {
            format!(
                "could not fetch the auth service's key from {auth_service} ({e}); \
                 start it or set AUTH_PUBLIC_KEY"
            )
        })?;
    tracing::info!(%key, "using the auth service's token key");
    Ok(key)
}

/// `AUTH_SERVICE_URL`, or the auth service on this machine.
pub fn auth_service_url() -> String {
    std::env::var("AUTH_SERVICE_URL").unwrap_or_else(|_| "http://127.0.0.1:3002".to_string())
}

/// Serves `app` on `bind` until asked to stop, within `limits`, answering
/// web pages from the origins in `CORS_ORIGINS`; and `admin` on
/// `ADMIN_ADDR` and metrics on `METRICS_ADDR`, if set.
pub async fn serve(
    app: Router,
    admin: Router,
    bind: &str,
    name: &str,
    limits: &Limits,
) -> Result<(), Box<dyn Error>> {
    if let Some(config) = admin::Admin::from_env()? {
        let listener = tokio::net::TcpListener::bind(&config.address).await?;
        tracing::info!(address = %config.address, "{name}'s admin API listening");
        let admin = observe::requests(config.guard(admin), false);
        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, admin).await {
                tracing::error!(error = %e, "serving the admin API failed");
            }
        });
    }
    let app = match std::env::var("METRICS_ADDR") {
        Ok(address) => {
            let measured = measure::endpoint(measure::install(name)?);
            let listener = tokio::net::TcpListener::bind(&address).await?;
            tracing::info!(%address, "{name}'s metrics at /metrics");
            tokio::spawn(async move {
                if let Err(e) = axum::serve(listener, measured).await {
                    tracing::error!(error = %e, "serving metrics failed");
                }
            });
            measure::requests(app)
        }
        Err(_) => app,
    };
    let app = observe::requests(limits.apply(app.layer(cors()?)), limits.trust_proxy);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, limits = limits.enabled(), "{name} listening");
    // Each request knows its client's address, for the limits.
    let app = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(stopping())
        .await?;
    tracing::info!("{name} stopped");
    observe::flush();
    Ok(())
}

/// Resolves when the service is asked to stop: Ctrl-C, or SIGTERM, which
/// is how Kubernetes and compose ask. Requests in progress then finish;
/// open WebSockets close, and their clients reconnect elsewhere.
async fn stopping() {
    let interrupted = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminated = async {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                terminate.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "can't listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminated = std::future::pending::<()>();
    tokio::select! {
        () = interrupted => {}
        () = terminated => {}
    }
    tracing::info!("stopping");
}

/// What an error response says: the error itself, unless it's internal
/// (a 500), which is logged and not shown.
pub fn error_message(status: StatusCode, error: &dyn Display) -> String {
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        tracing::error!(%error, "internal error");
        "internal server error".to_string()
    } else {
        error.to_string()
    }
}

/// An error response: `{"error": "..."}` with `status` (see
/// [`error_message`]).
pub fn error_response(status: StatusCode, error: &dyn Display) -> Response {
    (
        status,
        Json(json!({ "error": error_message(status, error) })),
    )
        .into_response()
}

/// Which web pages may call the service from a browser, from
/// `CORS_ORIGINS`: a comma-separated list of origins (such as
/// `https://dnd.example.com`), or `*` for any (for development). Unset,
/// none may: `edda` doesn't need it, and neither does a web page
/// served from the same address as the services (behind the proxy).
fn cors() -> Result<CorsLayer, String> {
    let setting = std::env::var("CORS_ORIGINS").unwrap_or_default();
    cors_for(&setting)
}

fn cors_for(setting: &str) -> Result<CorsLayer, String> {
    let origins: Vec<&str> = setting
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .collect();
    if origins.is_empty() {
        return Ok(CorsLayer::new());
    }
    let allowed = if origins == ["*"] {
        AllowOrigin::any()
    } else {
        let parsed: Result<Vec<HeaderValue>, _> = origins
            .iter()
            .map(|o| {
                HeaderValue::from_str(o).map_err(|_| format!("CORS_ORIGINS: {o:?} isn't an origin"))
            })
            .collect();
        AllowOrigin::list(parsed?)
    };
    // Tokens travel in the Authorization header, not cookies, so no
    // credentials are needed.
    Ok(CorsLayer::new()
        .allow_origin(allowed)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers([header::ETAG, header::LOCATION]))
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn it_is_not_ready_while_its_database_cant_be_reached() {
        // Nothing listens on port 1.
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://postgres@127.0.0.1:1/nothing")
            .unwrap();
        let res = readiness(pool)
            .oneshot(Request::get("/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn a_direct_url_is_the_pooled_one_unless_given() {
        let pooled = "postgres://pgbouncer:6432/combats";
        assert_eq!(direct_or(pooled, None), pooled);
        assert_eq!(direct_or(pooled, Some(String::new())), pooled);
        let direct = "postgres://postgres:5432/combats";
        assert_eq!(direct_or(pooled, Some(direct.into())), direct);
    }

    #[test]
    fn origins_are_a_list_or_any() {
        assert!(cors_for("").is_ok());
        assert!(cors_for("https://dnd.example.com, http://192.168.1.20:8080").is_ok());
        assert!(cors_for("*").is_ok());
        assert!(cors_for("https://bad\norigin").is_err());
    }
}
