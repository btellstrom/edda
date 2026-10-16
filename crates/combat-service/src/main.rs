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

    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://combats.db".to_string());
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3001".to_string());

    let pool = combat_service::connect(&database_url).await?;
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%bind_addr, %database_url, "combat service listening");

    axum::serve(listener, combat_service::app(pool))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
