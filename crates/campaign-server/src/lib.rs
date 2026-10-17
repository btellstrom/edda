//! What every campaign-manager service shares.

use axum::http::{HeaderValue, header};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

/// Which web pages may call the service from a browser, from
/// `CORS_ORIGINS`: a comma-separated list of origins (such as
/// `https://dnd.example.com`), or `*` for any (for development). Unset,
/// none may: `campaign-edit` doesn't need it, and neither does a web page
/// served from the same address as the services (behind the proxy).
pub fn cors() -> Result<CorsLayer, String> {
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

/// Writes a consistent copy of the database to `to`, which mustn't exist
/// yet, while the service keeps running (SQLite's `VACUUM INTO`).
#[cfg(feature = "sqlite")]
pub async fn backup(pool: &sqlx::SqlitePool, to: &std::path::Path) -> Result<(), sqlx::Error> {
    sqlx::query("VACUUM INTO ?")
        .bind(to.to_string_lossy())
        .execute(pool)
        .await
        .map(drop)
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
