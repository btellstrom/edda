#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! End-to-end tests of the limits a service open to others keeps: how
//! often a client may try to sign in, and how much a campaign may keep.

use campaign_auth::{DeviceCert, DeviceKey, Role, server::TokenVerifier};
use campaign_client::{ClientError, combat::CombatClient};
use campaign_server::Limits;
use campaign_types::combat::{CombatCommand, Combatant, CommandEnvelope, ServerMessage};
use reqwest::StatusCode;

mod support;

use support::{Auth, database_url};

#[tokio::test]
async fn signing_in_is_limited_per_address_and_devices_per_campaign() {
    let mut limits = Limits::on();
    limits.sign_ins_per_minute = 3;
    limits.devices = 2;
    // As behind the proxy: only requests that came through it, saying
    // where from, are counted.
    limits.trust_proxy = true;
    let auth = Auth::start_with(limits).await;
    let party = auth.campaign().await;

    let http = reqwest::Client::new();
    let challenge = |from: &'static str| {
        http.post(format!("{}/login/challenge", auth.url))
            .header("x-forwarded-for", format!("203.0.113.9, {from}"))
            .json(&serde_json::json!({}))
            .send()
    };
    for _ in 0..3 {
        assert_ne!(
            challenge("192.0.2.7").await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    let refused = challenge("192.0.2.7").await.unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.headers().contains_key("retry-after"));
    assert_ne!(
        challenge("192.0.2.8").await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // Only signing in: a device with a token carries on.
    let signed_in = http
        .get(format!("{}/campaign", auth.url))
        .header("x-forwarded-for", "192.0.2.7")
        .bearer_auth(&party.dm.token)
        .send()
        .await
        .unwrap();
    assert_eq!(signed_in.status(), StatusCode::OK);

    // The DM's device and one more fill the campaign.
    party.player(&auth, "Laura's phone").await;
    let key = DeviceKey::generate();
    let cert = DeviceCert::issue(
        &party.dm_key,
        party.id(),
        key.public(),
        Role::Player,
        "Travis",
    )
    .unwrap();
    let refused = auth
        .client()
        .with_token(&party.dm.token)
        .add_device(&cert)
        .await
        .unwrap_err();
    assert!(
        matches!(&refused, ClientError::Forbidden(m) if m == "this campaign has reached its limit of 2 devices"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_campaign_keeps_only_so_many_combats_events_and_watchers() {
    let auth = Auth::start().await;
    let party = auth.campaign().await;
    let mut limits = Limits::on();
    limits.combats = 1;
    limits.events_per_combat = 3;
    limits.connections_per_combat = 1;
    let pool = combat_service::connect(&database_url("combat_test").await)
        .await
        .unwrap();
    let config = combat_service::Config {
        limits,
        ..Default::default()
    };
    let app = combat_service::app(pool, TokenVerifier::new(auth.key), config)
        .await
        .unwrap();
    let combats = CombatClient::new(support::serve(app).await).with_token(&party.dm.token);

    let combat = combats.create("Ambush").await.unwrap().id;
    let refused = combats.create("Another").await.unwrap_err();
    assert!(matches!(&refused, ClientError::Forbidden(m) if m.contains("limit of 1 combats")));

    // Created, and two more events: then it's full.
    let add = |name: &str| {
        CommandEnvelope::new(CombatCommand::AddCombatant {
            combatant: Combatant::new(name, 7),
        })
    };
    combats.command(combat, &add("Goblin")).await.unwrap();
    combats.command(combat, &add("Orc")).await.unwrap();
    let refused = combats.command(combat, &add("Rat")).await.unwrap_err();
    assert!(
        matches!(&refused, ClientError::Forbidden(m) if m.contains("limit of 3 events")),
        "{refused:?}"
    );

    // Watching once the snapshot has come.
    let mut watching = combats.connect(combat).await.unwrap();
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), watching.recv()).await;
    assert!(matches!(first, Ok(Some(Ok(ServerMessage::Snapshot(_))))));
    assert!(
        combats.connect(combat).await.is_err(),
        "one watcher at a time"
    );
}
