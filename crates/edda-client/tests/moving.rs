#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! Moving a whole campaign from one server to another through the admin
//! API, as a hosting moves a customer between cells: exported from one,
//! imported into the other just as it was, and deleted from the first.

use edda_auth::{DeviceKey, PublicKey, server::TokenVerifier};
use edda_client::{
    Client, ClientError,
    admin::{AdminClient, Services},
    auth::AuthClient,
    combat::CombatClient,
    monsters::MonsterClient,
};
use edda_server::{Limits, admin::Admin, testing::fresh_database_url};
use edda_types::{
    CharacterSheet, Edition,
    combat::{CombatCommand, Combatant, CommandEnvelope},
    encounter::EncounterPlan,
    monster::MonsterSheet,
};
use uuid::Uuid;

mod support;

const TOKEN: &str = "the-admin-token-of-this-test-0123456789";

/// A whole set of services, on databases of their own.
struct Cell {
    auth: String,
    characters: String,
    combat: String,
    monsters: String,
    admin: Services,
}

impl Cell {
    /// One signing tokens with `token_key`, as every cell of a hosting
    /// does with the same one.
    async fn start(token_key: &DeviceKey) -> Self {
        let admin = Admin::new("unused", TOKEN).unwrap();
        let serve_admin =
            |router| async { AdminClient::new(support::serve(admin.guard(router)).await, TOKEN) };

        let pool = auth_service::connect(&fresh_database_url("auth_test").await)
            .await
            .unwrap();
        let auth_admin = serve_admin(auth_service::admin::router(pool.clone())).await;
        let app = auth_service::app(
            pool,
            Some(DeviceKey::from_seed(token_key.seed())),
            Limits::off(),
        );
        let auth = support::serve(app.await.unwrap()).await;
        let key = AuthClient::new(&auth).server_key().await.unwrap();

        let pool = character_service::connect(&fresh_database_url("characters_test").await)
            .await
            .unwrap();
        let characters_admin = serve_admin(character_service::admin::router(pool.clone())).await;
        let characters = support::serve(character_service::app(
            pool,
            TokenVerifier::new(key),
            Limits::off(),
        ))
        .await;

        let pool = combat_service::connect(&fresh_database_url("combat_test").await)
            .await
            .unwrap();
        let combat_admin = serve_admin(combat_service::admin::router(pool.clone())).await;
        let app = combat_service::app(
            pool,
            TokenVerifier::new(key),
            combat_service::Config::default(),
        );
        let combat = support::serve(app.await.unwrap()).await;

        let pool = monster_service::connect(&fresh_database_url("monsters_test").await)
            .await
            .unwrap();
        let monsters_admin = serve_admin(monster_service::admin::router(pool.clone())).await;
        let monsters = support::serve(monster_service::app(
            pool,
            TokenVerifier::new(key),
            Limits::off(),
        ))
        .await;

        Self {
            auth,
            characters,
            combat,
            monsters,
            admin: Services {
                auth: auth_admin,
                characters: characters_admin,
                combat: combat_admin,
                monsters: monsters_admin,
            },
        }
    }

    /// The device's token there. It insists on the token key it first saw,
    /// as a device does, so it logs in only where that key signs.
    async fn login(&self, campaign: Uuid, key: &DeviceKey, server_key: PublicKey) -> String {
        let client = AuthClient::new(&self.auth);
        client
            .login(campaign, key, Some(server_key))
            .await
            .unwrap()
            .token
    }
}

#[tokio::test]
async fn a_campaign_moves_whole_from_one_server_to_another() {
    let token_key = DeviceKey::generate();
    let (old, new) = (Cell::start(&token_key).await, Cell::start(&token_key).await);

    // A campaign with something in every service.
    let created = old
        .admin
        .auth
        .create_campaign("Vox Machina", Edition::Rules2024)
        .await
        .unwrap();
    let campaign = created.campaign.id;
    let dm_key = DeviceKey::generate();
    AuthClient::new(&old.auth)
        .setup(&created.setup_code, &dm_key, "DM laptop")
        .await
        .unwrap();
    let token = old.login(campaign, &dm_key, token_key.public()).await;
    let grog = Client::new(&old.characters)
        .with_token(&token)
        .create(&CharacterSheet {
            name: "Grog".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let combats = CombatClient::new(&old.combat).with_token(&token);
    let fight = combats.create("Ambush").await.unwrap().id;
    let goblin = Combatant::new("Goblin", 7);
    let goblin_id = goblin.id;
    combats
        .command(
            fight,
            &CommandEnvelope::new(CombatCommand::AddCombatant { combatant: goblin }),
        )
        .await
        .unwrap();
    combats
        .create_encounter(&EncounterPlan {
            name: "Bridge".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    MonsterClient::new(&old.monsters)
        .with_token(&token)
        .create(&MonsterSheet {
            name: "Cave Troll".into(),
            hit_points: 84,
            ..Default::default()
        })
        .await
        .unwrap();

    let plan = edda_auth::protocol::CampaignSettings {
        quotas: edda_auth::protocol::Quotas {
            characters: Some(5),
            ..Default::default()
        },
        ..Default::default()
    };
    old.admin.auth.settings(campaign, &plan).await.unwrap();

    let bundle = old.admin.export_campaign(campaign).await.unwrap();
    new.admin.import_campaign(&bundle).await.unwrap();
    old.admin.delete_campaign(campaign).await.unwrap();
    assert!(matches!(
        old.admin.auth.export::<serde_json::Value>(campaign).await,
        Err(ClientError::NotFound)
    ));

    // Everything as it was, for the same device, trusting the same key.
    assert_eq!(new.admin.auth.get_settings(campaign).await.unwrap(), plan);
    let token = new.login(campaign, &dm_key, token_key.public()).await;
    let characters = Client::new(&new.characters).with_token(&token);
    assert_eq!(characters.get(grog.id).await.unwrap(), grog);
    let combats = CombatClient::new(&new.combat).with_token(&token);
    assert_eq!(
        combats.log(fight, 0).await.unwrap(),
        bundle.combat.combats[0].log
    );
    assert_eq!(combats.encounters().await.unwrap()[0].plan.name, "Bridge");
    let troll = MonsterClient::new(&new.monsters).with_token(&token);
    assert_eq!(
        troll.list(Some("troll")).await.unwrap()[0].name,
        "Cave Troll"
    );
    // The device's own commands are still its own to undo.
    combats
        .command(
            fight,
            &CommandEnvelope::new(CombatCommand::Undo { command: None }),
        )
        .await
        .unwrap();
    let snapshot = combats.snapshot(fight).await.unwrap();
    assert!(
        snapshot.state.combatant(goblin_id).is_none(),
        "the goblin's arrival undone"
    );

    // Once is enough.
    let again = new.admin.import_campaign(&bundle).await;
    assert!(matches!(again, Err(ClientError::Conflict(_))), "{again:?}");
    assert_eq!(characters.list().await.unwrap().len(), 1, "nothing doubled");
}

#[tokio::test]
async fn a_campaign_that_cant_all_come_leaves_nothing_behind() {
    let token_key = DeviceKey::generate();
    let (old, new) = (Cell::start(&token_key).await, Cell::start(&token_key).await);
    let created = old
        .admin
        .auth
        .create_campaign("Mighty Nein", Edition::Rules2014)
        .await
        .unwrap();
    let campaign = created.campaign.id;
    let dm_key = DeviceKey::generate();
    AuthClient::new(&old.auth)
        .setup(&created.setup_code, &dm_key, "DM laptop")
        .await
        .unwrap();
    let token = old.login(campaign, &dm_key, token_key.public()).await;
    CombatClient::new(&old.combat)
        .with_token(&token)
        .create("Ambush")
        .await
        .unwrap();

    // A log with a gap in it can't be taken in.
    let mut bundle = old.admin.export_campaign(campaign).await.unwrap();
    bundle.combat.combats[0].log[0].seq = 7;
    let failed = new.admin.import_campaign(&bundle).await;
    assert!(
        matches!(failed, Err(ClientError::Rejected(_))),
        "{failed:?}"
    );
    assert!(matches!(
        new.admin.auth.export::<serde_json::Value>(campaign).await,
        Err(ClientError::NotFound)
    ));
}
