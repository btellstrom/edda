use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    campaign_server::init_logging("character_service");

    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://characters.db".to_string());

    // `character-service delete-campaign <ID>` deletes a campaign's
    // characters (`make delete-campaign` deletes the rest of it too).
    let args: Vec<String> = env::args().skip(1).collect();
    if let [command, campaign] = args.as_slice()
        && command == "delete-campaign"
    {
        let campaign: uuid::Uuid = campaign.parse()?;
        let pool = character_service::connect(&database_url).await?;
        let deleted = character_service::CharacterStore::new(pool)
            .delete_campaign(campaign)
            .await?;
        println!("Deleted {deleted} character(s) of campaign {campaign}");
        return Ok(());
    }
    // `character-service backup <PATH>` writes a copy of the database to
    // PATH, while the service runs (tools/backup.sh).
    if let [command, path] = args.as_slice()
        && command == "backup"
    {
        let pool = character_service::connect(&database_url).await?;
        campaign_server::backup(&pool, std::path::Path::new(path)).await?;
        return Ok(());
    }
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());

    let verifier = campaign_auth::server::TokenVerifier::new(
        campaign_server::auth_key(&campaign_server::auth_service_url()).await?,
    );
    let pool = character_service::connect(&database_url).await?;

    let limits = campaign_server::Limits::from_env()?;
    let app = character_service::app(pool, verifier, limits.clone());
    campaign_server::serve(app, &bind_addr, "character service", &limits).await
}
