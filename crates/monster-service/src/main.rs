use std::{env, path::PathBuf};

use campaign_types::{
    monster::MonsterSheet,
    rules_text::{ConditionText, RulesText, SpellText},
};
use clap::{Parser, Subcommand, ValueEnum};
use monster_service::MonsterStore;
use serde::de::DeserializeOwned;
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
    /// Load stat blocks (or with --kind, spells or conditions) from JSON
    /// files, each a list of them or just one, into the shared library, or
    /// with --campaign into a campaign's own. Ones already there (same
    /// name, source and edition) are updated.
    Import {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        campaign: Option<Uuid>,
        #[arg(long, value_enum, default_value_t = Kind::Monsters)]
        kind: Kind,
    },
    /// Delete a campaign's own monsters (`make delete-campaign` deletes
    /// the rest of the campaign too).
    DeleteCampaign { campaign: Uuid },
    /// Write a copy of the database to a new file, while the service runs
    /// (tools/backup.sh).
    Backup { to: PathBuf },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Kind {
    Monsters,
    Spells,
    Conditions,
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
        Some(Command::Import {
            files,
            campaign,
            kind,
        }) => {
            let store = MonsterStore::new(pool);
            for file in files {
                let text = std::fs::read_to_string(&file)
                    .map_err(|e| format!("reading {}: {e}", file.display()))?;
                let imported = match kind {
                    Kind::Monsters => {
                        let sheets = parse::<MonsterSheet>(&text)
                            .map_err(|e| format!("{}: {e}", file.display()))?;
                        store.import(campaign, &sheets).await
                    }
                    Kind::Spells => import_texts::<SpellText>(&store, campaign, &text)
                        .await
                        .map_err(|e| format!("{}: {e}", file.display()))?,
                    Kind::Conditions => import_texts::<ConditionText>(&store, campaign, &text)
                        .await
                        .map_err(|e| format!("{}: {e}", file.display()))?,
                }
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
        Some(Command::Backup { to }) => Ok(campaign_server::backup(&pool, &to).await?),
    }
}

/// A list of them, or a single one.
fn parse<T: DeserializeOwned>(text: &str) -> Result<Vec<T>, serde_json::Error> {
    match serde_json::from_str::<Vec<T>>(text) {
        Ok(all) => Ok(all),
        Err(_) => serde_json::from_str::<T>(text).map(|one| vec![one]),
    }
}

async fn import_texts<T: RulesText>(
    store: &MonsterStore,
    campaign: Option<Uuid>,
    text: &str,
) -> Result<
    Result<monster_service::store::Imported, monster_service::error::ApiError>,
    serde_json::Error,
> {
    let texts = parse::<T>(text)?;
    Ok(store.import_texts(campaign, &texts).await)
}

async fn serve(pool: sqlx::SqlitePool, bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let verifier = campaign_auth::server::TokenVerifier::new(auth_key().await?);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tracing::info!(%bind, "monster service listening");
    let app = monster_service::app(pool, verifier).layer(campaign_server::cors()?);
    axum::serve(listener, app)
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
