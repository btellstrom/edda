use std::{env, path::PathBuf};

use campaign_types::{
    monster::MonsterSheet,
    rules_text::{ConditionText, RulesText, SpellText},
};
use clap::{Parser, Subcommand, ValueEnum};
use monster_service::MonsterStore;
use serde::de::DeserializeOwned;
use uuid::Uuid;

#[derive(Parser)]
#[command(about = "Campaign manager monster service")]
struct Cli {
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://postgres:postgres@localhost:5432/monsters"
    )]
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
    /// Bring the monsters, spells and conditions over from the SQLite
    /// database the service used to keep (make migrate-sqlite).
    ImportSqlite { file: PathBuf },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Kind {
    Monsters,
    Spells,
    Conditions,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    campaign_server::init_logging("monster_service");
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
        Some(Command::ImportSqlite { file }) => {
            let imported = monster_service::sqlite::import_sqlite(&pool, &file).await?;
            println!(
                "Imported {} monster(s) and {} spell(s) and condition(s) from {}",
                imported.monsters,
                imported.rules_texts,
                file.display()
            );
            Ok(())
        }
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

async fn serve(pool: sqlx::PgPool, bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let verifier = campaign_auth::server::TokenVerifier::new(
        campaign_server::auth_key(&campaign_server::auth_service_url()).await?,
    );
    let limits = campaign_server::Limits::from_env()?;
    let app = monster_service::app(pool, verifier, limits.clone());
    campaign_server::serve(app, bind, "monster service", &limits).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_holds_a_list_or_just_one() {
        let many: Vec<MonsterSheet> =
            parse(r#"[{"name": "Goblin", "hit_points": 7}, {"name": "Orc", "hit_points": 15}]"#)
                .unwrap();
        assert_eq!(many.len(), 2);
        let one: Vec<SpellText> = parse(r#"{"name": "Firebolt", "level": 0}"#).unwrap();
        assert_eq!(one[0].name, "Firebolt");
        assert!(parse::<MonsterSheet>("not json").is_err());
        // A list with a broken entry is refused, not half imported.
        assert!(parse::<MonsterSheet>(r#"[{"name": 7}]"#).is_err());
    }
}
