//! What every edda service shares: logging, opening its
//! database, finding the auth service's key, serving (with the CORS setting
//! and the limits), and error responses.

pub mod limits;
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
use sqlx::{PgPool, Postgres, migrate::MigrateDatabase, postgres::PgPoolOptions};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tracing_subscriber::EnvFilter;

/// Logs to stderr: `crate_name`'s own messages, this crate's (serving,
/// internal errors) and each request, at info, unless `RUST_LOG` says
/// otherwise.
pub fn init_logging(crate_name: &str) {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!("{crate_name}=info,edda_server=info,tower_http=info").into()
        }))
        .init();
}

/// A connection pool to the Postgres database at `url`, creating the
/// database first if there's none yet. The service then applies its own
/// migrations (sqlx serialises them with an advisory lock, so instances
/// may start at once).
pub async fn open_database(url: &str) -> Result<PgPool, sqlx::Error> {
    if !Postgres::database_exists(url).await? {
        // Another instance may be creating it at the same moment.
        if let Err(e) = Postgres::create_database(url).await
            && !Postgres::database_exists(url).await?
        {
            return Err(e);
        }
    }
    PgPoolOptions::new().connect(url).await
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

/// Serves `app` on `bind` until Ctrl-C, within `limits`, answering web
/// pages from the origins in `CORS_ORIGINS`.
pub async fn serve(
    app: Router,
    bind: &str,
    name: &str,
    limits: &Limits,
) -> Result<(), Box<dyn Error>> {
    let app = limits.apply(app.layer(cors()?));
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, limits = limits.enabled(), "{name} listening");
    // Each request knows its client's address, for the limits.
    let app = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
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
    use super::*;

    #[test]
    fn origins_are_a_list_or_any() {
        assert!(cors_for("").is_ok());
        assert!(cors_for("https://dnd.example.com, http://192.168.1.20:8080").is_ok());
        assert!(cors_for("*").is_ok());
        assert!(cors_for("https://bad\norigin").is_err());
    }
}
