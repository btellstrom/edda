//! Runs the client against a real character service on a random port.

use campaign_client::{Client, ClientError};
use campaign_types::{CharacterAction, CharacterSheet, Pool, Recharge, Resource};
use sqlx::sqlite::SqlitePoolOptions;

async fn server() -> Client {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    character_service::migrate(&pool).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, character_service::app(pool)).into_future());
    Client::new(format!("http://{addr}/"))
}

fn grog() -> CharacterSheet {
    CharacterSheet {
        name: "Grog".into(),
        resources: vec![Resource {
            name: "Rage".into(),
            uses: Pool::new(1),
            recharge: Recharge::LongRest,
            notes: String::new(),
        }],
        ..Default::default()
    }
}

#[tokio::test]
async fn crud_and_actions_round_trip() {
    let client = server().await;
    let created = client.create(&grog()).await.unwrap();
    assert_eq!(client.list().await.unwrap()[0].name, "Grog");

    let raged = client
        .act(
            created.id,
            &CharacterAction::UseResource {
                name: "rage".into(),
                amount: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(raged.sheet.resources[0].uses.used, 1);

    let err = client
        .act(
            created.id,
            &CharacterAction::UseResource {
                name: "rage".into(),
                amount: 1,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Exhausted(_)), "{err:?}");

    client.delete(created.id).await.unwrap();
    assert!(matches!(
        client.get(created.id).await,
        Err(ClientError::NotFound)
    ));
}

#[tokio::test]
async fn stale_replace_reports_current_and_modify_merges() {
    let client = server().await;
    let created = client.create(&grog()).await.unwrap();

    // Someone else rages after we fetched.
    client
        .act(
            created.id,
            &CharacterAction::UseResource {
                name: "Rage".into(),
                amount: 1,
            },
        )
        .await
        .unwrap();

    let mut stale = created.sheet.clone();
    stale.notes = "hello".into();
    match client.replace(created.id, created.version, &stale).await {
        Err(ClientError::Stale(current)) => assert_eq!(current.version, 2),
        other => panic!("expected stale, got {other:?}"),
    }

    // modify() retries on the newer copy, so the rage stays spent.
    let saved = client
        .modify(created.id, |sheet| {
            sheet.notes = "hello".into();
            Ok::<_, ClientError>(())
        })
        .await
        .unwrap();
    assert_eq!(saved.sheet.notes, "hello");
    assert_eq!(saved.sheet.resources[0].uses.used, 1);
}

#[tokio::test]
async fn invalid_edits_are_rejected() {
    let client = server().await;
    let created = client.create(&grog()).await.unwrap();
    let err = client
        .modify(created.id, |sheet| {
            sheet.name.clear();
            Ok::<_, ClientError>(())
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, ClientError::Rejected(ref m) if m.contains("name")),
        "{err:?}"
    );
}

#[tokio::test]
async fn spell_slots_survive_the_round_trip() {
    // Regression: slot levels are JSON map keys, which failed to decode
    // into Character over HTTP.
    let client = server().await;
    let mut sheet = grog();
    sheet.spellcasting.slots.insert(1, Pool::new(2));
    sheet.spellcasting.slots.insert(3, Pool::new(1));
    let created = client.create(&sheet).await.unwrap();
    let fetched = client.get(created.id).await.unwrap();
    assert_eq!(fetched.sheet.spellcasting.slots, sheet.spellcasting.slots);

    let after = client
        .act(created.id, &CharacterAction::ExpendSpellSlot { level: 3 })
        .await
        .unwrap();
    assert_eq!(after.sheet.spellcasting.slots[&3].used, 1);
}
