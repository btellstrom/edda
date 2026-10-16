use std::{env, path::PathBuf};

use campaign_types::monster::MonsterSheet;
use clap::{Parser, Subcommand};
use monster_service::MonsterStore;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Parser)]
#[command(about = "Campaign manager monster service")]
struct Cli {
    #[arg(long, env = "DATABASE_URL", default_value = "sqlite://monsters.db")]
    database_url: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the service (the default).
    Serve {
        #[arg(long, env = "BIND_ADDR", default_value = "127.0.0.1:3003")]
        bind: String,
    },
    /// Load stat blocks from JSON files (each a list of them, or just one)
    /// into the shared library, or with --campaign into a campaign's own.
    /// Ones already there (same name, source and edition) are updated.
    Import {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        campaign: Option<Uuid>,
    },
    /// Delete a campaign's own monsters (`make delete-campaign` deletes
    /// the rest of the campaign too).
    DeleteCampaign { campaign: Uuid },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "monster_service=info,tower_http=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let pool = monster_service::connect(&cli.database_url).await?;

    match cli.command {
        None => {
            serve(
                pool,
                &env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3003".into()),
            )
            .await
        }
        Some(Command::Serve { bind }) => serve(pool, &bind).await,
        Some(Command::Import { files, campaign }) => {
            let store = MonsterStore::new(pool);
            for file in files {
                let text = std::fs::read_to_string(&file)
                    .map_err(|e| format!("reading {}: {e}", file.display()))?;
                let sheets = parse(&text).map_err(|e| format!("{}: {e}", file.display()))?;
                let imported = store
                    .import(campaign, &sheets)
                    .await
                    .map_err(|e| format!("{}: {e}", file.display()))?;
                let into = campaign.map_or("the shared library".to_string(), |c| {
                    format!("campaign {c}")
                });
                println!(
                    "{}: {} added, {} updated in {into}",
                    file.display(),
                    imported.added,
                    imported.updated
                );
            }
            Ok(())
        }
        Some(Command::DeleteCampaign { campaign }) => {
            let deleted = MonsterStore::new(pool).delete_campaign(campaign).await?;
            println!("Deleted {deleted} monster(s) of campaign {campaign}");
            Ok(())
        }
    }
}

/// A list of stat blocks, or a single one.
fn parse(text: &str) -> Result<Vec<MonsterSheet>, serde_json::Error> {
    match serde_json::from_str::<Vec<MonsterSheet>>(text) {
        Ok(sheets) => Ok(sheets),
        Err(_) => serde_json::from_str::<MonsterSheet>(text).map(|s| vec![s]),
    }
}

async fn serve(pool: sqlx::SqlitePool, bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let verifier = campaign_auth::server::TokenVerifier::new(auth_key().await?);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "monster service listening");
    axum::serve(listener, monster_service::app(pool, verifier))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// The key tokens are signed with: `AUTH_PUBLIC_KEY` if set, otherwise
/// fetched from the auth service at `AUTH_SERVICE_URL`.
async fn auth_key() -> Result<campaign_auth::PublicKey, Box<dyn std::error::Error>> {
    if let Ok(key) = env::var("AUTH_PUBLIC_KEY") {
        return Ok(key.parse()?);
    }
    let url = env::var("AUTH_SERVICE_URL").unwrap_or_else(|_| "http://127.0.0.1:3002".to_string());
    let key = campaign_client::auth::AuthClient::new(&url)
        .wait_for_server_key(std::time::Duration::from_secs(30))
        .await
        .map_err(|e| {
            format!("could not fetch the auth service's key from {url} ({e}); start it or set AUTH_PUBLIC_KEY")
        })?;
    tracing::info!(%key, "using the auth service's token key");
    Ok(key)
}
