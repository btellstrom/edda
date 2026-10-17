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
