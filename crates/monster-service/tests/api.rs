use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use campaign_auth::{
    DeviceKey,
    protocol::{Claims, Principal},
    server::{TokenSigner, TokenVerifier},
};
use campaign_types::{
    Edition,
    monster::MonsterSheet,
    rules_text::{ConditionText, SpellText},
};
use chrono::Utc;
use http_body_util::BodyExt;
use monster_service::MonsterStore;
use serde_json::{Value, json};
use sqlx::sqlite::SqlitePoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

/// The service and its store, with tokens signed for it.
struct App {
    router: Router,
    store: MonsterStore,
    signer: TokenSigner,
}

/// Calls the service as one campaign.
struct As<'a> {
    app: &'a App,
    token: String,
}

impl App {
    async fn new() -> App {
        // An in-memory database lives per connection, so pin the pool to one.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        monster_service::migrate(&pool).await.unwrap();
        let signer = TokenSigner::new(&DeviceKey::generate());
        App {
            router: monster_service::app(pool.clone(), TokenVerifier::new(signer.public_key())),
            store: MonsterStore::new(pool),
            signer,
        }
    }

    fn campaign(&self, campaign: Uuid, edition: Edition) -> As<'_> {
        let token = self
            .signer
            .issue(&Claims {
                key: DeviceKey::generate().public(),
                campaign,
                role: Principal::Player,
                name: "Test".into(),
                edition,
                expires_at: Utc::now() + chrono::Duration::minutes(5),
            })
            .unwrap();
        As { app: self, token }
    }
}

impl As<'_> {
    async fn send(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token));
        let body = match body {
            Some(v) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let res = self
            .app
            .router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, json)
    }

    /// The names in the monster list.
    async fn names(&self, query: &str) -> Vec<String> {
        let (status, list) = self
            .send(Method::GET, &format!("/monsters{query}"), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        list.as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_string())
            .collect()
    }
}

fn sheet(name: &str, source: &str, edition: Option<Edition>) -> MonsterSheet {
    MonsterSheet {
        name: name.into(),
        hit_points: 7,
        hit_dice: "2d6".into(),
        source: source.into(),
        edition,
        ..Default::default()
    }
}

/// A shared library with a monster for each edition and one for either.
async fn library(app: &App) {
    let shared = [
        sheet("Goblin", "SRD 5.1", Some(Edition::Rules2014)),
        sheet("Goblin Warrior", "SRD 5.2", Some(Edition::Rules2024)),
        sheet("Rat", "Homebrew", None),
    ];
    app.store.import(None, &shared).await.unwrap();
}

#[tokio::test]
async fn campaigns_see_the_shared_monsters_of_their_edition() {
    let app = App::new().await;
    library(&app).await;
    let old = app.campaign(Uuid::new_v4(), Edition::Rules2014);
    let new = app.campaign(Uuid::new_v4(), Edition::Rules2024);
    assert_eq!(old.names("").await, ["Goblin", "Rat"]);
    assert_eq!(new.names("").await, ["Goblin Warrior", "Rat"]);
    assert_eq!(new.names("?search=gob").await, ["Goblin Warrior"]);

    let (_, list) = new.send(Method::GET, "/monsters", None).await;
    assert_eq!(list[0]["shared"], true);
    let id = list[0]["id"].as_str().unwrap();
    let (status, goblin) = new
        .send(Method::GET, &format!("/monsters/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(goblin["hit_dice"], "2d6");
    assert!(goblin.get("campaign").is_none(), "shared");
}

#[tokio::test]
async fn campaigns_keep_their_own_monsters_and_cannot_change_shared_ones() {
    let app = App::new().await;
    library(&app).await;
    let campaign = Uuid::new_v4();
    let ours = app.campaign(campaign, Edition::Rules2024);
    let theirs = app.campaign(Uuid::new_v4(), Edition::Rules2024);

    let (status, created) = ours
        .send(
            Method::POST,
            "/monsters",
            Some(json!({ "name": "Cave Troll", "hit_points": 84, "hit_dice": "8d10+40" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["source"], "Homebrew");
    let uri = format!("/monsters/{}", created["id"].as_str().unwrap());
    assert_eq!(
        ours.names("").await,
        ["Cave Troll", "Goblin Warrior", "Rat"]
    );
    assert_eq!(theirs.names("").await, ["Goblin Warrior", "Rat"]);
    assert_eq!(
        theirs.send(Method::GET, &uri, None).await.0,
        StatusCode::NOT_FOUND
    );

    let mut troll = created.clone();
    troll["armor_class"] = json!(15);
    let (status, saved) = ours.send(Method::PUT, &uri, Some(troll)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (saved["armor_class"].as_u64(), saved["version"].as_u64()),
        (Some(15), Some(2))
    );
    assert_eq!(
        theirs.send(Method::DELETE, &uri, None).await.0,
        StatusCode::NOT_FOUND
    );

    let (status, _) = ours
        .send(
            Method::POST,
            "/monsters",
            Some(json!({ "name": "Blob", "hit_dice": "lots" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (_, list) = ours.send(Method::GET, "/monsters?search=rat", None).await;
    let rat = format!("/monsters/{}", list[0]["id"].as_str().unwrap());
    let (status, body) = ours.send(Method::DELETE, &rat, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = ours
        .send(
            Method::PUT,
            &rat,
            Some(json!({ "name": "My Rat", "hit_points": 1 })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    assert_eq!(
        ours.send(Method::DELETE, &uri, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(ours.names("").await, ["Goblin Warrior", "Rat"]);
}

#[tokio::test]
async fn importing_again_updates_and_campaigns_can_be_deleted() {
    let app = App::new().await;
    let mut library: Vec<MonsterSheet> = (1..=50)
        .map(|n| {
            sheet(
                &format!("Monster {n}"),
                "Bestiary",
                Some(Edition::Rules2024),
            )
        })
        .collect();
    library.push(sheet("Troll", "Bestiary", Some(Edition::Rules2024)));
    library.push(sheet("Troll Limb", "Bestiary", Some(Edition::Rules2024)));
    let first = app.store.import(None, &library).await.unwrap();
    assert_eq!((first.added, first.updated), (52, 0));
    library[0].armor_class = 18;
    let again = app.store.import(None, &library).await.unwrap();
    assert_eq!((again.added, again.updated), (0, 52));

    let campaign = Uuid::new_v4();
    let ours = app.campaign(campaign, Edition::Rules2024);
    assert_eq!(ours.names("").await.len(), 52);
    let (_, list) = ours
        .send(Method::GET, "/monsters?search=monster%201", None)
        .await;
    assert_eq!(list[0]["armor_class"], 18, "updated in place");
    let homebrew = [sheet("Cave Troll", "Homebrew", None)];
    app.store.import(Some(campaign), &homebrew).await.unwrap();
    assert_eq!(
        ours.names("?search=troll").await,
        ["Cave Troll", "Troll", "Troll Limb"]
    );

    // A bad stat block anywhere in a file imports nothing.
    let mut broken = homebrew.to_vec();
    broken.push(sheet("", "Homebrew", None));
    assert!(app.store.import(Some(campaign), &broken).await.is_err());

    assert_eq!(app.store.delete_campaign(campaign).await.unwrap(), 1);
    assert_eq!(
        ours.names("?search=troll").await,
        ["Troll", "Troll Limb"],
        "the shared library stays"
    );
}

fn spell(name: &str, level: u8, edition: Option<Edition>) -> SpellText {
    SpellText {
        name: name.into(),
        level,
        description: format!("What {name} does."),
        source: "SRD".into(),
        edition,
        ..Default::default()
    }
}

#[tokio::test]
async fn spells_and_conditions_are_kept_like_monsters() {
    let app = App::new().await;
    let shared = [
        spell("Bless", 1, Some(Edition::Rules2014)),
        spell("Bless", 1, Some(Edition::Rules2024)),
        spell("Fireball", 3, None),
    ];
    let imported = app.store.import_texts(None, &shared).await.unwrap();
    assert_eq!(imported.added, 3);
    let prone = ConditionText {
        name: "Prone".into(),
        description: "- Crawl.".into(),
        source: "SRD".into(),
        edition: None,
    };
    app.store.import_texts(None, &[prone]).await.unwrap();

    let campaign = Uuid::new_v4();
    let ours = app.campaign(campaign, Edition::Rules2024);
    let (status, spells) = ours.send(Method::GET, "/spells", None).await;
    assert_eq!(status, StatusCode::OK);
    let editions: Vec<_> = spells
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["edition"].clone()))
        .collect();
    assert_eq!(
        editions,
        [("Bless", json!("2024")), ("Fireball", Value::Null)],
        "the campaign's edition, whole entries"
    );
    assert_eq!(spells[1]["description"], "What Fireball does.");
    let (_, conditions) = ours.send(Method::GET, "/conditions?search=pro", None).await;
    assert_eq!(conditions[0]["name"], "Prone");

    // Homebrew: added, changed and deleted by the campaign; the shared
    // library can't be changed.
    let (status, zap) = ours
        .send(
            Method::POST,
            "/spells",
            Some(json!({"name": "Zap", "level": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let zap_id = zap["id"].as_str().unwrap();
    let (status, changed) = ours
        .send(
            Method::PUT,
            &format!("/spells/{zap_id}"),
            Some(json!({"name": "Zap", "level": 2})),
        )
        .await;
    assert_eq!(
        (status, changed["level"].clone()),
        (StatusCode::OK, json!(2))
    );
    let (status, _) = ours
        .send(
            Method::POST,
            "/spells",
            Some(json!({"name": "Too much", "level": 10})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let shared_id = spells[1]["id"].as_str().unwrap();
    let (status, _) = ours
        .send(Method::DELETE, &format!("/spells/{shared_id}"), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let others = app.campaign(Uuid::new_v4(), Edition::Rules2024);
    let (status, _) = others
        .send(Method::GET, &format!("/spells/{zap_id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another campaign's");
    let (status, _) = ours
        .send(Method::GET, &format!("/conditions/{zap_id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a spell isn't a condition");

    app.store.delete_campaign(campaign).await.unwrap();
    let (status, _) = ours
        .send(Method::GET, &format!("/spells/{zap_id}"), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "deleted with the campaign");
}
