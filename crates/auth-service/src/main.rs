use std::env;

use campaign_auth::PublicKey;
use campaign_types::Edition;
use clap::{Parser, Subcommand};
use uuid::Uuid;

/// How long a setup code stays usable.
const SETUP_CODE_LIFETIME: chrono::Duration = chrono::Duration::hours(24);

#[derive(Parser)]
#[command(about = "Campaign manager auth service")]
struct Cli {
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://postgres:postgres@localhost:5432/auth"
    )]
    database_url: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the service (the default).
    Serve {
        #[arg(long, env = "BIND_ADDR", default_value = "127.0.0.1:3002")]
        bind: String,
    },
    /// Create a campaign and print the one-time code for its DM to set up
    /// their first device with.
    NewCampaign {
        name: String,
        /// Which rules it plays by: 2014 or 2024.
        #[arg(long)]
        edition: Edition,
    },
    /// Print a new DM setup code for an existing campaign, e.g. when every
    /// DM device has been lost.
    SetupCode { campaign: Uuid },
    /// Delete a campaign with its devices. Its characters and combats live
    /// in the other services: `make delete-campaign` removes everything.
    DeleteCampaign { campaign: Uuid },
    /// Let a service log in to any campaign with this key.
    AddService { name: String, public_key: PublicKey },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    campaign_server::init_logging("auth_service");
    let cli = Cli::parse();
    let pool = auth_service::connect(&cli.database_url).await?;
    let store = auth_service::Store::new(pool.clone());

    match cli.command {
        None => {
            serve(
                pool,
                &env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3002".into()),
            )
            .await?
        }
        Some(Command::Serve { bind }) => serve(pool, &bind).await?,
        Some(Command::NewCampaign { name, edition }) => {
            let campaign = store.create_campaign(&name, edition).await?;
            println!(
                "Created campaign {} ({}), playing by the {} rules",
                campaign.name, campaign.id, campaign.edition
            );
            print_setup_code(&store, campaign.id).await?;
        }
        Some(Command::SetupCode { campaign }) => print_setup_code(&store, campaign).await?,
        Some(Command::DeleteCampaign { campaign }) => {
            match store.delete_campaign(campaign).await? {
                Some((deleted, devices)) => println!(
                    "Deleted campaign {} ({campaign}) and its {devices} device(s)",
                    deleted.name
                ),
                None => println!("No campaign {campaign} in the auth service"),
            }
        }
        Some(Command::AddService { name, public_key }) => {
            store.add_service(&name, public_key).await?;
            println!("Service {name} may now log in with key {public_key}");
        }
    }
    Ok(())
}

async fn serve(pool: sqlx::PgPool, bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let app = auth_service::app(pool).await?;
    campaign_server::serve(app, bind, "auth service").await
}

async fn print_setup_code(
    store: &auth_service::Store,
    campaign: Uuid,
) -> Result<(), Box<dyn std::error::Error>> {
    let code = store.new_setup_code(campaign, SETUP_CODE_LIFETIME).await?;
    println!("DM setup code (valid for 24 hours, works once):\n\n    {code}\n");
    println!("On the DM's device:  campaign-edit auth setup {code} --name \"<device name>\"");
    Ok(())
}
