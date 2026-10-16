use std::env;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "combat_service=info,tower_http=info".into()),
        )
        .init();

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/combats".to_string());
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3001".to_string());
    // Set CHARACTER_SERVICE_URL to an empty string to turn HP write-back off.
    let character_service =
        env::var("CHARACTER_SERVICE_URL").unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());
    let config = combat_service::Config {
        character_service: Some(character_service).filter(|url| !url.is_empty()),
    };

    let pool = combat_service::connect(&database_url).await?;
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%bind_addr, ?config, "combat service listening");

    axum::serve(listener, combat_service::app(pool, config).await?)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
