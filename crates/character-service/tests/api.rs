#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;

use campaign_auth::{
    DeviceKey, PublicKey,
    protocol::{Claims, Principal},
    server::{TokenSigner, TokenVerifier},
};
use campaign_types::Edition;
use chrono::Utc;
use tower::ServiceExt;
use uuid::Uuid;

/// The service, plus a token for one campaign that it accepts, for one
/// device: a player's, unless asked otherwise.
#[derive(Clone)]
struct App {
    router: Router,
    signer: Arc<TokenSigner>,
    key: PublicKey,
    role: Principal,
    campaign: Uuid,
    token: Option<String>,
}

impl App {
    fn new(pool: sqlx::SqlitePool) -> App {
        let signer = Arc::new(TokenSigner::new(&DeviceKey::generate()));
        let verifier = TokenVerifier::new(signer.public_key());
        let app = App {
            router: character_service::app(pool, verifier),
            signer,
            key: DeviceKey::generate().public(),
            role: Principal::Player,
            campaign: Uuid::nil(),
            token: None,
        };
        app.as_campaign(Some(Uuid::new_v4()))
    }

    /// The same service and device, with a token for some other campaign
    /// (or none).
    fn as_campaign(&self, campaign: Option<Uuid>) -> App {
        let mut app = self.clone();
        app.campaign = campaign.unwrap_or(self.campaign);
        app.token = campaign.map(|c| app.token(c, Edition::default()));
        app
    }

    /// The same device, for a campaign playing by `edition`'s rules.
    fn with_edition(&self, campaign: Uuid, edition: Edition) -> App {
        let mut app = self.clone();
        app.campaign = campaign;
        app.token = Some(app.token(campaign, edition));
        app
    }

    /// Another device in the same campaign: another player's, or a DM's.
    fn another(&self, role: Principal) -> App {
        let mut app = self.clone();
        app.key = DeviceKey::generate().public();
        app.role = role;
        app.token = Some(app.token(self.campaign, Edition::default()));
        app
    }

    fn token(&self, campaign: Uuid, edition: Edition) -> String {
        self.signer
            .issue(&Claims {
                key: self.key,
                campaign,
                role: self.role,
                name: "Test".into(),
                edition,
                expires_at: Utc::now() + chrono::Duration::minutes(5),
            })
            .unwrap()
    }
}

async fn test_app() -> App {
    // An in-memory database lives per connection, so pin the pool to one.
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    character_service::migrate(&pool).await.unwrap();
    App::new(pool)
}

async fn send(app: &App, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (status, _, json) = request(app, method, uri, None, body).await;
    (status, json)
}

/// PUT with an optional `If-Match` header.
async fn put(
    app: &App,
    uri: &str,
    if_match: Option<&str>,
    body: Value,
) -> (StatusCode, HeaderMap, Value) {
    request(app, Method::PUT, uri, if_match, Some(body)).await
}

async fn request(
    app: &App,
    method: Method,
    uri: &str,
    if_match: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = &app.token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(tag) = if_match {
        req = req.header(header::IF_MATCH, tag);
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app
        .router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, headers, json)
}

fn sample() -> Value {
    json!({
        "name": "Grog",
        "player_name": "Travis",
        "race": "Goliath",
        "classes": [{ "name": "Barbarian", "level": 3 }],
        "ability_scores": { "strength": 18, "constitution": 16 },
        "hit_points": { "max": 35, "current": 35 },
        "skill_proficiencies": ["athletics", "intimidation"],
        "inventory": [{ "name": "Greataxe" }]
    })
}

#[tokio::test]
async fn full_crud_lifecycle() {
    let app = test_app().await;

    let (status, created) = send(&app, Method::POST, "/characters", Some(sample())).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["name"], "Grog");
    assert_eq!(
        created["ability_scores"]["dexterity"], 10,
        "defaults applied"
    );
    assert_eq!(created["inventory"][0]["quantity"], 1);

    let (status, list) = send(&app, Method::GET, "/characters", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["level"], 3);

    let (status, fetched) = send(&app, Method::GET, &format!("/characters/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched, created);

    let mut updated = sample();
    updated["hit_points"]["current"] = json!(12);
    let (status, headers, replaced) =
        put(&app, &format!("/characters/{id}"), Some("\"1\""), updated).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replaced["hit_points"]["current"], 12);
    assert_eq!(replaced["version"], 2);
    assert_eq!(headers[header::ETAG], "\"2\"");
    assert_eq!(replaced["created_at"], created["created_at"]);

    let (status, _) = send(&app, Method::DELETE, &format!("/characters/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = send(&app, Method::GET, &format!("/characters/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rejects_invalid_sheets() {
    let app = test_app().await;
    let mut bad = sample();
    bad["ability_scores"]["wisdom"] = json!(31);
    let (status, body) = send(&app, Method::POST, "/characters", Some(bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["error"].as_str().unwrap().contains("Wisdom"));
}

#[tokio::test]
async fn missing_characters_are_404() {
    let app = test_app().await;
    let uri = format!("/characters/{}", uuid::Uuid::new_v4());
    assert_eq!(
        send(&app, Method::GET, &uri, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        put(&app, &uri, Some("\"1\""), sample()).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, Method::DELETE, &uri, None).await.0,
        StatusCode::NOT_FOUND
    );
}

fn caster() -> Value {
    json!({
        "name": "Hexblade Knight",
        "classes": [
            { "name": "Fighter", "subclass": "Eldritch Knight", "level": 3,
              "spellcasting_ability": "intelligence" },
            { "name": "Warlock", "level": 2, "spellcasting_ability": "charisma" }
        ],
        "spellcasting": {
            "spells": [
                { "name": "Fire Bolt", "level": 0 },
                { "name": "Shield", "level": 1, "class": "Fighter" },
                { "name": "Hex", "level": 1, "class": "Warlock", "concentration": true }
            ]
        },
        "resources": [
            { "name": "Action Surge", "max": 1, "recharge": "short_rest" },
            { "name": "Second Wind", "max": 1, "recharge": "short_rest" }
        ]
    })
}

#[tokio::test]
async fn spell_slots_follow_the_classes_under_the_campaigns_edition() {
    // Regression: slots were entered by hand, so a sheet could have any
    // number of them whatever its classes.
    let app = test_app().await;
    let campaign = Uuid::new_v4();
    let rules_2014 = app.with_edition(campaign, Edition::Rules2014);
    let rules_2024 = app.with_edition(campaign, Edition::Rules2024);
    let slot = |max: u16, used: u16| json!({ "max": max, "used": used });

    // Wizard 3 / Cleric 2 share the slots of a 5th-level caster; slots
    // typed in by hand are ignored.
    let multiclass = json!({
        "name": "Pike",
        "classes": [
            { "name": "Wizard", "level": 3 },
            { "name": "Cleric", "level": 2 }
        ],
        "spellcasting": { "slots": { "9": { "max": 9 } } }
    });
    let (status, c) = send(&rules_2024, Method::POST, "/characters", Some(multiclass)).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        c["spellcasting"]["slots"],
        json!({ "1": slot(4, 0), "2": slot(3, 0), "3": slot(2, 0) })
    );

    // A 1st-level Ranger has no slots under the 2014 rules and two under
    // 2024's: the campaign's edition decides.
    let ranger = json!({ "name": "Vex", "classes": [{ "name": "Ranger", "level": 1 }] });
    let (_, c) = send(&rules_2014, Method::POST, "/characters", Some(ranger)).await;
    assert_eq!(c["spellcasting"]["slots"], json!({}));
    let uri = format!("/characters/{}", c["id"].as_str().unwrap());
    let (_, c) = send(&rules_2024, Method::GET, &uri, None).await;
    assert_eq!(c["spellcasting"]["slots"], json!({ "1": slot(2, 0) }));

    // Levelling up adds slots and keeps the spent ones spent.
    let (status, _) = send(
        &rules_2024,
        Method::POST,
        &format!("{uri}/actions"),
        Some(json!({ "type": "expend_spell_slot", "level": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut sheet = send(&rules_2024, Method::GET, &uri, None).await.1;
    sheet["classes"][0]["level"] = json!(5);
    let (status, _, c) = put(&rules_2024, &uri, Some("*"), sheet).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        c["spellcasting"]["slots"],
        json!({ "1": slot(4, 1), "2": slot(2, 0) })
    );
}

#[tokio::test]
async fn actions_spend_and_recover_resources() {
    let app = test_app().await;
    let (_, created) = send(&app, Method::POST, "/characters", Some(caster())).await;
    let actions = format!("/characters/{}/actions", created["id"].as_str().unwrap());
    let act = |body: Value| send(&app, Method::POST, &actions, Some(body));

    let (status, c) = act(json!({ "type": "cast_spell", "spell": "shield" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(c["spellcasting"]["slots"]["1"]["used"], 1);

    let (status, c) = act(json!({ "type": "cast_spell", "spell": "Hex", "pact": true })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(c["spellcasting"]["pact_magic"]["used"], 1);

    let (status, c) = act(json!({ "type": "use_resource", "name": "Action Surge" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(c["resources"][0]["used"], 1);

    // Out of uses -> 409; nonsense -> 422.
    let (status, body) = act(json!({ "type": "use_resource", "name": "Action Surge" })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().contains("0 uses remaining"));
    let (status, _) = act(json!({ "type": "cast_spell", "spell": "Wish" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, c) = act(json!({ "type": "rest", "kind": "short" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(c["resources"][0]["used"], 0);
    assert_eq!(c["spellcasting"]["pact_magic"]["used"], 0);
    assert_eq!(
        c["spellcasting"]["slots"]["1"]["used"], 1,
        "needs a long rest"
    );

    // Changes are persisted, not just echoed back.
    let (_, fetched) = send(&app, Method::GET, &actions.replace("/actions", ""), None).await;
    assert_eq!(fetched, c);
}

#[tokio::test]
async fn an_action_with_an_idempotency_key_is_applied_once() {
    let app = test_app().await;
    let (_, created) = send(&app, Method::POST, "/characters", Some(caster())).await;
    let actions = format!("/characters/{}/actions", created["id"].as_str().unwrap());
    let act = |key: &'static str, body: Value| {
        let req = Request::builder()
            .method(Method::POST)
            .uri(&actions)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", app.token.as_ref().unwrap()),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .header("Idempotency-Key", key)
            .body(Body::from(body.to_string()))
            .unwrap();
        let router = app.router.clone();
        async move {
            let res = router.oneshot(req).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap())
        }
    };
    let spend = json!({ "type": "expend_spell_slot", "level": 1 });
    let (status, c) = act("cast-1", spend.clone()).await;
    assert_eq!(
        (status, c["spellcasting"]["slots"]["1"]["used"].clone()),
        (StatusCode::OK, json!(1))
    );
    let (status, c) = act("cast-1", spend.clone()).await;
    assert_eq!(
        (status, c["spellcasting"]["slots"]["1"]["used"].clone()),
        (StatusCode::OK, json!(1)),
        "a retry changes nothing"
    );
    let (_, c) = act("cast-2", spend).await;
    assert_eq!(
        c["spellcasting"]["slots"]["1"]["used"], 2,
        "another key is another action"
    );
    // An action that fails isn't remembered, so it can be tried again.
    let (status, _) = act(
        "surge",
        json!({ "type": "use_resource", "name": "Action Surge", "amount": 5 }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, c) = act(
        "surge",
        json!({ "type": "use_resource", "name": "Action Surge" }),
    )
    .await;
    assert_eq!(
        (status, c["resources"][0]["used"].clone()),
        (StatusCode::OK, json!(1))
    );
}

#[tokio::test]
async fn concurrent_actions_never_overspend() {
    // Needs a real file so the pool can hold several connections.
    let path = std::env::temp_dir().join(format!("cm-test-{}.db", uuid::Uuid::new_v4()));
    let pool = character_service::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let app = App::new(pool);

    let mut sheet = caster();
    sheet["resources"][0]["max"] = json!(5);
    let (_, created) = send(&app, Method::POST, "/characters", Some(sheet)).await;
    let uri = format!("/characters/{}/actions", created["id"].as_str().unwrap());

    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let (app, uri) = (app.clone(), uri.clone());
            tokio::spawn(async move {
                let body = json!({ "type": "use_resource", "name": "Action Surge" });
                send(&app, Method::POST, &uri, Some(body)).await.0
            })
        })
        .collect();
    let mut ok = 0;
    for task in tasks {
        match task.await.unwrap() {
            StatusCode::OK => ok += 1,
            StatusCode::CONFLICT => {}
            other => panic!("unexpected status {other}"),
        }
    }
    assert_eq!(ok, 5);

    let (_, c) = send(&app, Method::GET, &uri.replace("/actions", ""), None).await;
    assert_eq!(c["resources"][0]["used"], 5);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[tokio::test]
async fn stale_put_cannot_undo_actions() {
    let app = test_app().await;
    let (status, headers, created) =
        request(&app, Method::POST, "/characters", None, Some(caster())).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(headers[header::ETAG], "\"1\"");
    let uri = format!("/characters/{}", created["id"].as_str().unwrap());

    // A player spends Action Surge from their phone...
    let (_, after_action) = send(
        &app,
        Method::POST,
        &format!("{uri}/actions"),
        Some(json!({ "type": "use_resource", "name": "Action Surge" })),
    )
    .await;
    assert_eq!(after_action["version"], 2);

    // ...while the DM edits notes on the copy they loaded at version 1.
    let mut stale = created.clone();
    stale["notes"] = json!("Owes the Slayer's Take 50gp");
    let (status, headers, body) = put(&app, &uri, Some("\"1\""), stale).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(headers[header::ETAG], "\"2\"");
    assert_eq!(
        body["current"], after_action,
        "server hands back the latest copy"
    );

    // The DM re-applies the edit to the latest copy and it goes through,
    // with the spent Action Surge intact.
    let mut merged = body["current"].clone();
    merged["notes"] = json!("Owes the Slayer's Take 50gp");
    let (status, _, saved) = put(&app, &uri, Some("\"2\""), merged).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["version"], 3);
    assert_eq!(saved["resources"][0]["used"], 1);
    assert_eq!(saved["notes"], "Owes the Slayer's Take 50gp");
}

#[tokio::test]
async fn put_requires_a_valid_if_match() {
    let app = test_app().await;
    let (_, created) = send(&app, Method::POST, "/characters", Some(sample())).await;
    let uri = format!("/characters/{}", created["id"].as_str().unwrap());

    let (status, _, _) = put(&app, &uri, None, sample()).await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);

    for bad in ["1", "W/\"1\"", "\"one\"", "\"1\", \"2\""] {
        let (status, _, _) = put(&app, &uri, Some(bad), sample()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "If-Match: {bad}");
    }

    // A version that could never exist is just stale, not a server error.
    let (status, _, _) = put(&app, &uri, Some("\"18446744073709551615\""), sample()).await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED);

    // `*` deliberately opts out of the check.
    let (status, _, body) = put(&app, &uri, Some("*"), sample()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], 2);

    // Nothing was written by the rejected attempts.
    let (_, fetched) = send(&app, Method::GET, &uri, None).await;
    assert_eq!(fetched["version"], 2);
}

#[tokio::test]
async fn requests_need_a_valid_token() {
    let app = test_app().await;
    let anonymous = app.as_campaign(None);
    let (status, _) = send(&anonymous, Method::GET, "/characters", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // A token from some other auth service is worthless here.
    let forged = App {
        signer: Arc::new(TokenSigner::new(&DeviceKey::generate())),
        ..app.clone()
    }
    .as_campaign(Some(Uuid::new_v4()));
    let (status, body) = send(&forged, Method::POST, "/characters", Some(sample())).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "invalid or expired token");
}

#[tokio::test]
async fn campaigns_only_see_their_own_characters() {
    let ours = test_app().await;
    let theirs = ours.as_campaign(Some(Uuid::new_v4()));
    let (_, grog) = send(&ours, Method::POST, "/characters", Some(sample())).await;
    let uri = format!("/characters/{}", grog["id"].as_str().unwrap());

    let (_, list) = send(&theirs, Method::GET, "/characters", None).await;
    assert_eq!(list, json!([]));
    for (method, body) in [
        (Method::GET, None),
        (Method::DELETE, None),
        (Method::PUT, Some(sample())),
    ] {
        let (status, _, _) = request(&theirs, method.clone(), &uri, Some("*"), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }
    let action = json!({ "type": "rest", "kind": "long" });
    let (status, _) = send(
        &theirs,
        Method::POST,
        &format!("{uri}/actions"),
        Some(action),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(&ours, Method::GET, &uri, None).await;
    assert_eq!(status, StatusCode::OK, "still there for its own campaign");
}

#[tokio::test]
async fn deleting_a_campaign_deletes_only_its_characters() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    character_service::migrate(&pool).await.unwrap();
    let app = App::new(pool.clone());
    let campaign = Uuid::new_v4();
    let doomed = app.as_campaign(Some(campaign));
    let other = app.as_campaign(Some(Uuid::new_v4()));
    for app in [&doomed, &doomed, &other] {
        let (status, _) = send(app, Method::POST, "/characters", Some(sample())).await;
        assert_eq!(status, StatusCode::CREATED);
    }

    let store = character_service::CharacterStore::new(pool);
    assert_eq!(store.delete_campaign(campaign).await.unwrap(), 2);
    assert_eq!(
        send(&doomed, Method::GET, "/characters", None).await.1,
        json!([])
    );
    let (_, kept) = send(&other, Method::GET, "/characters", None).await;
    assert_eq!(kept.as_array().unwrap().len(), 1);
    assert_eq!(store.delete_campaign(campaign).await.unwrap(), 0);
}

#[tokio::test]
async fn players_change_only_the_characters_their_device_plays() {
    let laura = test_app().await;
    let travis = laura.another(Principal::Player);
    let dm = laura.another(Principal::Dm);
    let (_, vex) = send(
        &laura,
        Method::POST,
        "/characters",
        Some(json!({ "name": "Vex" })),
    )
    .await;
    assert_eq!(vex["owners"], json!([laura.key.to_string()]));
    let uri = format!("/characters/{}", vex["id"].as_str().unwrap());
    let rest = json!({ "type": "rest", "kind": "long" });

    // Everyone sees it; only its player and the DM change it.
    assert_eq!(
        send(&travis, Method::GET, &uri, None).await.0,
        StatusCode::OK
    );
    let (status, body) = send(
        &travis,
        Method::POST,
        &format!("{uri}/actions"),
        Some(rest.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        body["error"],
        "Vex isn't yours; the DM or their player changes them"
    );
    assert_eq!(
        put(&travis, &uri, Some("*"), json!({ "name": "Vex" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&travis, Method::DELETE, &uri, None).await.0,
        StatusCode::FORBIDDEN
    );
    for app in [&laura, &dm] {
        let (status, _) = send(
            app,
            Method::POST,
            &format!("{uri}/actions"),
            Some(rest.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // The DM chooses who plays it; a DM-made character has nobody yet.
    let owners = format!("{uri}/owners");
    let both = json!([laura.key.to_string(), travis.key.to_string()]);
    assert_eq!(
        send(&travis, Method::PUT, &owners, Some(both.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, vex) = send(&dm, Method::PUT, &owners, Some(both.clone())).await;
    assert_eq!((status, &vex["owners"]), (StatusCode::OK, &both));
    let (status, _) = send(&travis, Method::POST, &format!("{uri}/actions"), Some(rest)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&dm, Method::PUT, &owners, Some(json!(["not a key"]))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, grog) = send(
        &dm,
        Method::POST,
        "/characters",
        Some(json!({ "name": "Grog" })),
    )
    .await;
    assert_eq!(grog["owners"], json!([]));
    let (_, list) = send(&travis, Method::GET, "/characters", None).await;
    let owners_of = |name: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["owners"]
            .clone()
    };
    assert_eq!((owners_of("Vex"), owners_of("Grog")), (both, json!([])));
}
