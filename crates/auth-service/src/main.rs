//! `auth-service`: serves the auth service, and the admin's commands
//! (`new-campaign`, `setup-code`, `add-service`, `delete-campaign`,
//! `new-token-key`).

use std::{env, path::PathBuf};

use clap::{Parser, Subcommand};
use edda_auth::{DeviceKey, PublicKey};
use edda_types::Edition;
use uuid::Uuid;

/// How long a setup code stays usable.

#[derive(Parser)]
#[command(about = "Edda auth service")]
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
    /// Write a new key for signing tokens to FILE, for `TOKEN_KEY_FILE`: one
    /// key that several auth services sign with, so a campaign's devices
    /// can log in on any of them.
    NewTokenKey { file: PathBuf },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    edda_server::init_logging("auth_service");
    let cli = Cli::parse();
    if let Some(Command::NewTokenKey { file }) = &cli.command {
        let key = DeviceKey::generate();
        key.save(file).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                format!("{} exists already; not writing over a key", file.display())
            }
            _ => format!("writing {}: {e}", file.display()),
        })?;
        println!(
            "Wrote a token key to {}; its public key is {}",
            file.display(),
            key.public()
        );
        return Ok(());
    }
    let pool = auth_service::connect(&cli.database_url).await?;
    edda_server::measure::watch_pool(&pool);
    let store = auth_service::Store::new(pool.clone());

    match cli.command {
        None => {
            serve(
                pool,
                &env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3002".into()),
            )
            .await?;
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
        Some(Command::NewTokenKey { .. }) => unreachable!("handled before connecting"),
        Some(Command::AddService { name, public_key }) => {
            store.add_service(&name, public_key).await?;
            println!("Service {name} may now log in with key {public_key}");
        }
    }
    Ok(())
}

async fn serve(pool: sqlx::PgPool, bind: &str) -> Result<(), Box<dyn std::error::Error>> {
    let limits = edda_server::Limits::from_env()?;
    let admin = auth_service::admin::router(pool.clone());
    let app = auth_service::app(pool, token_key()?, limits.clone()).await?;
    edda_server::serve(app, admin, bind, "auth service", &limits).await
}

/// The key in `TOKEN_KEY_FILE`, if it's set (see `new-token-key`).
fn token_key() -> Result<Option<DeviceKey>, Box<dyn std::error::Error>> {
    let Some(file) = env::var_os("TOKEN_KEY_FILE").filter(|f| !f.is_empty()) else {
        return Ok(None);
    };
    let file = PathBuf::from(file);
    let key = DeviceKey::load(&file)?.ok_or_else(|| {
        format!(
            "there's no {}; make one with: auth-service new-token-key {0}",
            file.display()
        )
    })?;
    tracing::info!(key = %key.public(), "signing tokens with the key in TOKEN_KEY_FILE");
    Ok(Some(key))
}

async fn print_setup_code(
    store: &auth_service::Store,
    campaign: Uuid,
) -> Result<(), Box<dyn std::error::Error>> {
    let code = store
        .new_setup_code(campaign, auth_service::admin::SETUP_CODE_LIFETIME)
        .await?;
    println!("DM setup code (valid for 24 hours, works once):\n\n    {code}\n");
    println!("On the DM's device:  edda auth setup {code} --name \"<device name>\"");
    Ok(())
}
