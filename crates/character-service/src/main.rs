use std::env;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "character_service=info,tower_http=info".into()),
        )
        .init();

    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://characters.db".to_string());
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());

    let verifier = campaign_auth::server::TokenVerifier::new(auth_key().await?);
    let pool = character_service::connect(&database_url).await?;
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%bind_addr, %database_url, "character service listening");

    axum::serve(listener, character_service::app(pool, verifier))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// The key tokens are signed with: `AUTH_PUBLIC_KEY` if set, otherwise
/// fetched from the auth service at `AUTH_SERVICE_URL`. Set the key
/// explicitly unless the network between the services is trusted.
async fn auth_key() -> Result<campaign_auth::PublicKey, Box<dyn std::error::Error>> {
    if let Ok(key) = env::var("AUTH_PUBLIC_KEY") {
        return Ok(key.parse()?);
    }
    let url = env::var("AUTH_SERVICE_URL").unwrap_or_else(|_| "http://127.0.0.1:3002".to_string());
    let key = campaign_client::auth::AuthClient::new(&url)
        .server_key()
        .await
        .map_err(|e| {
            format!("could not fetch the auth service's key from {url} ({e}); start it or set AUTH_PUBLIC_KEY")
        })?;
    tracing::info!(%key, "using the auth service's token key");
    Ok(key)
}
