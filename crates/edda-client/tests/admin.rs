#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! The admin API end to end: a campaign created, set up by its DM,
//! and deleted again from every service, as a hosting's control plane
//! does it.

use edda_auth::DeviceKey;
use edda_client::{Client, ClientError, admin::AdminClient};
use edda_server::{Limits, admin::Admin};
use edda_types::{CharacterSheet, Edition};

mod support;

use support::{Auth, database_url, serve};

const TOKEN: &str = "the-admin-token-of-this-test-0123456789";

/// `router` as its service's admin API, wanting [`TOKEN`].
async fn admin(router: axum::Router) -> AdminClient {
    let admin = Admin::new("unused", TOKEN).unwrap();
    AdminClient::new(serve(admin.guard(router)).await, TOKEN)
}

#[tokio::test]
async fn a_campaign_is_created_set_up_and_deleted_through_the_admin_api() {
    let auth = Auth::start().await;
    let auth_admin = admin(auth_service::admin::router(auth.store.pool().clone())).await;

    let created = auth_admin
        .create_campaign("  Vox Machina ", Edition::Rules2024)
        .await
        .unwrap();
    assert_eq!(created.campaign.name, "Vox Machina");
    assert_eq!(created.campaign.edition, Edition::Rules2024);

    // The DM sets up their device with the code, as `edda auth setup` does.
    let dm_key = DeviceKey::generate();
    let campaign = auth
        .client()
        .setup(&created.setup_code, &dm_key, "DM laptop")
        .await
        .unwrap();
    assert_eq!(campaign, created.campaign);
    let dm = auth.login(campaign.id, &dm_key).await;

    let pool = character_service::connect(&database_url("characters_test").await)
        .await
        .unwrap();
    let verifier = edda_auth::server::TokenVerifier::new(auth.key);
    let characters_admin = admin(character_service::admin::router(pool.clone())).await;
    let characters =
        Client::new(serve(character_service::app(pool, verifier, Limits::off())).await)
            .with_token(&dm.token);
    characters
        .create(&CharacterSheet {
            name: "Grog".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // A code anew, as when every DM device is lost.
    let again = auth_admin.setup_code(campaign.id).await.unwrap();
    assert_ne!(again, created.setup_code);

    assert_eq!(
        characters_admin.delete_campaign(campaign.id).await.unwrap(),
        1
    );
    assert_eq!(auth_admin.delete_campaign(campaign.id).await.unwrap(), 1);
    assert!(characters.list().await.unwrap().is_empty());
    let login = auth
        .client()
        .login(campaign.id, &dm_key, Some(auth.key))
        .await;
    assert!(
        matches!(login, Err(ClientError::Unauthorized(_))),
        "{login:?}"
    );
    // Deleting again does nothing, and a gone campaign has no codes.
    assert_eq!(auth_admin.delete_campaign(campaign.id).await.unwrap(), 0);
    assert!(matches!(
        auth_admin.setup_code(campaign.id).await,
        Err(ClientError::NotFound)
    ));
}

#[tokio::test]
async fn the_admin_api_wants_its_token_and_a_name() {
    let auth = Auth::start().await;
    let router = auth_service::admin::router(auth.store.pool().clone());
    let url = serve(Admin::new("unused", TOKEN).unwrap().guard(router)).await;

    let stranger = AdminClient::new(&url, "not-the-admin-token-but-long-enough-anyway");
    let refused = stranger
        .create_campaign("Vox Machina", Edition::Rules2024)
        .await;
    assert!(
        matches!(refused, Err(ClientError::Unauthorized(_))),
        "{refused:?}"
    );

    let unnamed = AdminClient::new(&url, TOKEN)
        .create_campaign("  ", Edition::Rules2014)
        .await;
    assert!(
        matches!(unnamed, Err(ClientError::Rejected(_))),
        "{unnamed:?}"
    );
}

#[tokio::test]
async fn a_campaign_keeps_what_its_settings_say_and_can_be_made_read_only() {
    use edda_auth::protocol::{CampaignSettings, Quotas};
    use edda_client::combat::CombatClient;
    use edda_types::combat::{CombatCommand, Combatant, CommandEnvelope, ServerMessage};

    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let auth_admin = admin(auth_service::admin::router(auth.store.pool().clone())).await;
    let settings = |read_only, characters| CampaignSettings {
        read_only,
        quotas: Quotas {
            characters,
            ..Quotas::default()
        },
    };

    let verifier = || edda_auth::server::TokenVerifier::new(auth.key);
    let pool = character_service::connect(&database_url("characters_test").await)
        .await
        .unwrap();
    let characters = serve(character_service::app(pool, verifier(), Limits::on())).await;
    let pool = combat_service::connect(&database_url("combat_test").await)
        .await
        .unwrap();
    let combat_app = combat_service::app(pool, verifier(), combat_service::Config::default());
    let combat = serve(combat_app.await.unwrap()).await;
    let grog = || CharacterSheet {
        name: "Grog".into(),
        ..Default::default()
    };

    // One character at most, from the next login on.
    auth_admin
        .settings(party.id(), &settings(false, Some(1)))
        .await
        .unwrap();
    assert_eq!(
        auth_admin.get_settings(party.id()).await.unwrap(),
        settings(false, Some(1))
    );
    let session = auth.login(party.id(), &party.dm_key).await;
    assert_eq!(session.claims.settings, settings(false, Some(1)));
    let dm = Client::new(&characters).with_token(&session.token);
    dm.create(&grog()).await.unwrap();
    let second = dm.create(&grog()).await;
    assert!(
        matches!(&second, Err(ClientError::Forbidden(m)) if m.contains("limit of 1 characters")),
        "{second:?}"
    );

    // Read-only: looking, but no changing, over HTTP or a WebSocket.
    let combats = CombatClient::new(&combat).with_token(&session.token);
    let fight = combats.create("Ambush").await.unwrap().id;
    auth_admin
        .settings(party.id(), &settings(true, None))
        .await
        .unwrap();
    let token = auth.login(party.id(), &party.dm_key).await.token;
    let dm = Client::new(&characters).with_token(&token);
    assert_eq!(dm.list().await.unwrap().len(), 1);
    let refused = dm.create(&grog()).await;
    assert!(
        matches!(&refused, Err(ClientError::Forbidden(m)) if m.contains("read-only")),
        "{refused:?}"
    );
    let combats = CombatClient::new(&combat).with_token(&token);
    let mut live = combats.connect(fight).await.unwrap();
    assert!(matches!(
        live.recv().await,
        Some(Ok(ServerMessage::Snapshot(_)))
    ));
    let add = CommandEnvelope::new(CombatCommand::AddCombatant {
        combatant: Combatant::new("Goblin", 7),
    });
    live.send(&add).await.unwrap();
    let answer = live.recv().await;
    assert!(
        matches!(&answer, Some(Ok(ServerMessage::Rejected { error, .. })) if error.contains("read-only")),
        "{answer:?}"
    );

    // And back.
    auth_admin
        .settings(party.id(), &CampaignSettings::default())
        .await
        .unwrap();
    let token = auth.login(party.id(), &party.dm_key).await.token;
    Client::new(&characters)
        .with_token(&token)
        .create(&grog())
        .await
        .unwrap();
}
