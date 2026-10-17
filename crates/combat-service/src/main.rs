use std::{env, path::Path, sync::Arc};

use campaign_auth::{DeviceKey, PublicKey, server::TokenVerifier};
use combat_service::services::ServicesConfig;

use tracing_subscriber::EnvFilter;

const DEFAULT_KEY_FILE: &str = "combat-service.key";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "combat_service=info,tower_http=info".into()),
        )
        .init();

    // `combat-service public-key` prints this service's key, creating it if
    // needed, so it can be registered: auth-service add-service combat-service <KEY>
    if env::args().nth(1).as_deref() == Some("public-key") {
        let key_file = env::var("SERVICE_KEY_FILE").unwrap_or_else(|_| DEFAULT_KEY_FILE.into());
        println!("{}", service_key(Path::new(&key_file))?.public());
        return Ok(());
    }

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/combats".to_string());

    // `combat-service delete-campaign <ID>` deletes a campaign's combats
    // (`make delete-campaign` deletes the rest of it too).
    let args: Vec<String> = env::args().skip(1).collect();
    if let [command, campaign] = args.as_slice()
        && command == "delete-campaign"
    {
        let campaign: uuid::Uuid = campaign.parse()?;
        let store =
            combat_service::store::Store::new(combat_service::connect(&database_url).await?);
        let deleted = store.delete_campaign(&campaign.to_string()).await?;
        println!("Deleted {deleted} combat(s) of campaign {campaign}");
        return Ok(());
    }
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3001".to_string());
    let auth_service =
        env::var("AUTH_SERVICE_URL").unwrap_or_else(|_| "http://127.0.0.1:3002".to_string());
    let auth_key = auth_key(&auth_service).await?;

    // Set CHARACTER_SERVICE_URL to an empty string to leave character sheets
    // alone (no HP written back, no slots or feature uses spent), and
    // MONSTER_SERVICE_URL to refuse adding monsters by id and fights from
    // plans.
    let other = |var: &str, default: &str| {
        Some(env::var(var).unwrap_or_else(|_| default.to_string())).filter(|url| !url.is_empty())
    };
    let character_service = other("CHARACTER_SERVICE_URL", "http://127.0.0.1:3000");
    let monster_service = other("MONSTER_SERVICE_URL", "http://127.0.0.1:3003");
    let services = if character_service.is_none() && monster_service.is_none() {
        None
    } else {
        let key_file =
            env::var("SERVICE_KEY_FILE").unwrap_or_else(|_| DEFAULT_KEY_FILE.to_string());
        Some(ServicesConfig {
            auth_service,
            auth_key,
            key: Arc::new(service_key(Path::new(&key_file))?),
            character_service,
            monster_service,
        })
    };
    let config = combat_service::Config { services };

    let pool = combat_service::connect(&database_url).await?;
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%bind_addr, ?config, "combat service listening");

    let app = combat_service::app(pool, TokenVerifier::new(auth_key), config).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

/// The key tokens are signed with: `AUTH_PUBLIC_KEY` if set, otherwise
/// fetched from the auth service. Set the key explicitly unless the network
/// between the services is trusted.
async fn auth_key(auth_service: &str) -> Result<PublicKey, Box<dyn std::error::Error>> {
    if let Ok(key) = env::var("AUTH_PUBLIC_KEY") {
        return Ok(key.parse()?);
    }
    let key = campaign_client::auth::AuthClient::new(auth_service)
        .wait_for_server_key(std::time::Duration::from_secs(30))
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

/// This service's own key, for reaching the other services. Created on
/// first start; it must then be registered with the auth service.
fn service_key(path: &Path) -> Result<DeviceKey, Box<dyn std::error::Error>> {
    let key = match DeviceKey::load(path)? {
        Some(key) => key,
        None => {
            let key = DeviceKey::generate();
            key.save(path)?;
            tracing::warn!(
                path = %path.display(),
                "created a service key; register it with: \
                 auth-service add-service combat-service {}",
                key.public()
            );
            key
        }
    };
    tracing::info!(key = %key.public(), "service key");
    Ok(key)
}
