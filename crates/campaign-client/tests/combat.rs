//! End-to-end tests against a real combat service.

use std::time::Duration;

use campaign_client::combat::{CombatClient, Received, Replica, live::CombatConnection};
use campaign_types::combat::{
    CombatCommand, CombatEvent, CombatState, CombatStatus, Combatant, CommandEnvelope, NewEffect,
    ServerMessage,
};
use uuid::Uuid;

/// Tests share one Postgres database (each works in its own campaign and
/// combats, so they don't interfere). Start one with `podman compose up -d`
/// or point `TEST_DATABASE_URL` elsewhere.
fn database_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/combat_test".to_string())
}

/// A unique campaign name, so listings only show this test's combats.
fn campaign() -> String {
    format!("test-{}", Uuid::new_v4())
}

/// One instance of the combat service. Starting several gives several
/// instances sharing the database, as behind a load balancer.
struct Server {
    client: CombatClient,
}

impl Server {
    async fn start() -> Self {
        Self::start_with(combat_service::Config::default()).await
    }

    async fn start_with(config: combat_service::Config) -> Self {
        let pool = combat_service::connect(&database_url())
            .await
            .expect("Postgres must be running for these tests (see README)");
        let app = combat_service::app(pool, config).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, app).into_future());
        Self {
            client: CombatClient::new(format!("http://{addr}")),
        }
    }
}

async fn run(client: &CombatClient, combat: Uuid, command: CombatCommand) -> Vec<CombatEvent> {
    let events = client
        .command(combat, &CommandEnvelope::new(command).by("DM"))
        .await
        .unwrap();
    events.into_iter().map(|e| e.event).collect()
}

async fn add(client: &CombatClient, combat: Uuid, name: &str, modifier: i8) -> Uuid {
    let mut c = Combatant::new(name, 20);
    c.initiative_modifier = modifier;
    let id = c.id;
    run(client, combat, CombatCommand::AddCombatant { combatant: c }).await;
    id
}

async fn next(conn: &mut CombatConnection) -> ServerMessage {
    tokio::time::timeout(Duration::from_secs(5), conn.recv())
        .await
        .expect("timed out waiting for the server")
        .expect("connection closed")
        .unwrap()
}

#[tokio::test]
async fn full_encounter_with_complete_log() {
    let server = Server::start().await;
    let api = &server.client;
    let campaign = campaign();
    let combat = api.create(&campaign, "Goblin ambush").await.unwrap().id;

    let vex = add(api, combat, "Vex", 4).await;
    let grog = add(api, combat, "Grog", 0).await;
    let goblin = add(api, combat, "Goblin", 2).await;

    // Players roll their own dice; the server rolls for the goblin.
    run(
        api,
        combat,
        CombatCommand::RollInitiative {
            combatant: vex,
            roll: Some(15),
        },
    )
    .await;
    run(
        api,
        combat,
        CombatCommand::RollInitiative {
            combatant: grog,
            roll: Some(3),
        },
    )
    .await;
    let rolled = run(api, combat, CombatCommand::RollInitiativeForAll).await;
    let [
        CombatEvent::InitiativeRolled {
            combatant,
            roll,
            total,
            ..
        },
    ] = rolled.as_slice()
    else {
        panic!("expected one server roll, got {rolled:?}");
    };
    assert_eq!(*combatant, goblin);
    assert!((1..=20).contains(roll));
    assert_eq!(*total, i16::from(*roll) + 2);

    run(api, combat, CombatCommand::Start).await;
    let first = api.snapshot(combat).await.unwrap().state.current_turn;
    run(
        api,
        combat,
        CombatCommand::AddEffect {
            effect: NewEffect {
                id: Uuid::new_v4(),
                name: "Hunter's Mark".into(),
                source: Some(vex),
                targets: vec![goblin],
                concentration: true,
                rounds: None,
            },
        },
    )
    .await;
    run(
        api,
        combat,
        CombatCommand::Damage {
            combatant: goblin,
            amount: 9,
        },
    )
    .await;
    run(api, combat, CombatCommand::NextTurn).await;

    // Current state: round, whose turn, initiative order, effects.
    let snapshot = api.snapshot(combat).await.unwrap();
    let state = &snapshot.state;
    assert_eq!(state.status, CombatStatus::Active);
    assert_eq!(state.round, 1);
    assert_ne!(state.current_turn, first);
    let order: Vec<_> = state.turn_order().map(|c| c.initiative.unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] >= w[1]), "sorted: {order:?}");
    assert_eq!(state.effects[0].name, "Hunter's Mark");
    assert_eq!(state.combatant(goblin).unwrap().hit_points.current, 11);

    // The log is complete, gapless, attributed, and rebuilds the state.
    let log = api.log(combat, 0).await.unwrap();
    let seqs: Vec<_> = log.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=snapshot.seq).collect::<Vec<_>>());
    assert!(matches!(log[0].event, CombatEvent::Created { .. }));
    assert!(log[1..].iter().all(|e| e.actor.as_deref() == Some("DM")));
    assert_eq!(&CombatState::replay(log.iter().map(|e| &e.event)), state);
    // And can be read incrementally.
    let tail = api.log(combat, snapshot.seq - 2).await.unwrap();
    assert_eq!(tail.len(), 2);

    let combats = api.list(Some(&campaign), None).await.unwrap();
    assert_eq!(
        (combats[0].status, combats[0].round),
        (CombatStatus::Active, 1)
    );
}

#[tokio::test]
async fn changes_are_pushed_to_every_connected_client() {
    let server = Server::start().await;
    let api = &server.client;
    let combat = api.create(&campaign(), "Live").await.unwrap().id;
    let goblin = add(api, combat, "Goblin", 0).await;

    let mut dm = api.connect(combat).await.unwrap();
    let mut player = api.connect(combat).await.unwrap();
    let ServerMessage::Snapshot(snapshot) = next(&mut player).await else {
        panic!("first message must be a snapshot");
    };
    let mut replica = Replica::new(snapshot);
    let ServerMessage::Snapshot(_) = next(&mut dm).await else {
        panic!("first message must be a snapshot");
    };

    // The player hits the goblin: shown locally at once...
    let hit = CommandEnvelope::new(CombatCommand::Damage {
        combatant: goblin,
        amount: 5,
    })
    .by("Vex");
    replica.submit(hit.clone()).unwrap();
    assert_eq!(
        replica
            .state()
            .combatant(goblin)
            .unwrap()
            .hit_points
            .current,
        15
    );
    player.send(&hit).await.unwrap();

    // ...confirmed back to the player...
    let message = next(&mut player).await;
    assert_eq!(replica.receive(message), Received::Updated);
    assert!(replica.pending().is_empty());
    assert_eq!(
        replica
            .confirmed()
            .combatant(goblin)
            .unwrap()
            .hit_points
            .current,
        15
    );

    // ...and pushed to the DM, attributed to the player.
    let ServerMessage::Event(event) = next(&mut dm).await else {
        panic!("expected an event");
    };
    assert_eq!(event.command_id, Some(hit.command_id));
    assert_eq!(event.actor.as_deref(), Some("Vex"));
    assert!(matches!(
        event.event,
        CombatEvent::Damaged { amount: 5, .. }
    ));

    // A rejected command is reported to its sender only.
    let bad = CommandEnvelope::new(CombatCommand::Heal {
        combatant: Uuid::new_v4(),
        amount: 1,
    });
    player.send(&bad).await.unwrap();
    let ServerMessage::Rejected { command_id, error } = next(&mut player).await else {
        panic!("expected a rejection");
    };
    assert_eq!(command_id, bad.command_id);
    assert!(error.contains("no combatant"), "{error}");

    // Changes over plain HTTP reach WebSocket clients too; the DM's next
    // message is this event, not the player's rejection.
    run(
        api,
        combat,
        CombatCommand::Heal {
            combatant: goblin,
            amount: 2,
        },
    )
    .await;
    let ServerMessage::Event(event) = next(&mut dm).await else {
        panic!("expected an event");
    };
    assert!(matches!(event.event, CombatEvent::Healed { amount: 2, .. }));
    assert!(replica.receive(next(&mut player).await) == Received::Updated);
    assert_eq!(
        replica
            .state()
            .combatant(goblin)
            .unwrap()
            .hit_points
            .current,
        17
    );
}

#[tokio::test]
async fn resent_commands_are_applied_once() {
    let server = Server::start().await;
    let api = &server.client;
    let combat = api.create(&campaign(), "Retry").await.unwrap().id;
    let goblin = add(api, combat, "Goblin", 0).await;

    let hit = CommandEnvelope::new(CombatCommand::Damage {
        combatant: goblin,
        amount: 5,
    });
    let first = api.command(combat, &hit).await.unwrap();
    let again = api.command(combat, &hit).await.unwrap();
    assert_eq!(first, again, "HTTP retry returns the original events");

    let mut conn = api.connect(combat).await.unwrap();
    next(&mut conn).await; // snapshot
    conn.send(&hit).await.unwrap();
    assert_eq!(
        next(&mut conn).await,
        ServerMessage::AlreadyApplied {
            command_id: hit.command_id
        }
    );

    let state = api.snapshot(combat).await.unwrap().state;
    assert_eq!(state.combatant(goblin).unwrap().hit_points.current, 15);
}

#[tokio::test]
async fn concurrent_commands_are_serialized() {
    let server = Server::start().await;
    let api = &server.client;
    let combat = api.create(&campaign(), "Melee").await.unwrap().id;
    let ogre = Combatant::new("Ogre", 100);
    let ogre_id = ogre.id;
    run(api, combat, CombatCommand::AddCombatant { combatant: ogre }).await;

    let hits: Vec<_> = (0..25)
        .map(|_| {
            let api = api.clone();
            tokio::spawn(async move {
                let hit = CombatCommand::Damage {
                    combatant: ogre_id,
                    amount: 3,
                };
                api.command(combat, &CommandEnvelope::new(hit))
                    .await
                    .unwrap()
            })
        })
        .collect();
    for hit in hits {
        hit.await.unwrap();
    }

    let snapshot = api.snapshot(combat).await.unwrap();
    assert_eq!(
        snapshot
            .state
            .combatant(ogre_id)
            .unwrap()
            .hit_points
            .current,
        25
    );
    let log = api.log(combat, 0).await.unwrap();
    assert_eq!(log.len() as u64, snapshot.seq);
    assert_eq!(
        log.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=snapshot.seq).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn unknown_combats_are_not_found() {
    let server = Server::start().await;
    let api = &server.client;
    let missing = Uuid::new_v4();
    assert!(matches!(
        api.snapshot(missing).await,
        Err(campaign_client::ClientError::NotFound)
    ));
    assert!(matches!(
        api.log(missing, 0).await,
        Err(campaign_client::ClientError::NotFound)
    ));
    assert!(api.connect(missing).await.is_err());
}

#[tokio::test]
async fn instances_share_combats() {
    let a = Server::start().await;
    let b = Server::start().await;
    let combat = a.client.create(&campaign(), "Split").await.unwrap().id;
    let ogre = Combatant::new("Ogre", 100);
    let ogre_id = ogre.id;
    run(
        &b.client,
        combat,
        CombatCommand::AddCombatant { combatant: ogre },
    )
    .await;

    // Clients connected to different instances see each other's changes.
    let mut on_a = a.client.connect(combat).await.unwrap();
    let mut on_b = b.client.connect(combat).await.unwrap();
    let ServerMessage::Snapshot(snapshot) = next(&mut on_a).await else {
        panic!("first message must be a snapshot");
    };
    assert!(snapshot.state.combatant(ogre_id).is_some());
    next(&mut on_b).await;
    let hit = CommandEnvelope::new(CombatCommand::Damage {
        combatant: ogre_id,
        amount: 4,
    });
    on_b.send(&hit).await.unwrap();
    for conn in [&mut on_a, &mut on_b] {
        let ServerMessage::Event(event) = next(conn).await else {
            panic!("expected an event");
        };
        assert_eq!(event.command_id, Some(hit.command_id));
    }

    // A command resent to the other instance is still applied only once.
    on_a.send(&hit).await.unwrap();
    assert_eq!(
        next(&mut on_a).await,
        ServerMessage::AlreadyApplied {
            command_id: hit.command_id
        }
    );

    // Concurrent commands spread over both instances are serialised.
    let hits: Vec<_> = (0..24)
        .map(|i| {
            let api = if i % 2 == 0 { &a.client } else { &b.client }.clone();
            tokio::spawn(async move {
                let hit = CombatCommand::Damage {
                    combatant: ogre_id,
                    amount: 3,
                };
                api.command(combat, &CommandEnvelope::new(hit))
                    .await
                    .unwrap()
            })
        })
        .collect();
    for hit in hits {
        hit.await.unwrap();
    }
    for api in [&a.client, &b.client] {
        let snapshot = api.snapshot(combat).await.unwrap();
        assert_eq!(
            snapshot
                .state
                .combatant(ogre_id)
                .unwrap()
                .hit_points
                .current,
            24
        );
        let seqs: Vec<_> = api
            .log(combat, 0)
            .await
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(seqs, (1..=snapshot.seq).collect::<Vec<_>>());
    }
    // Every event reached the clients on both instances, in order.
    for conn in [&mut on_a, &mut on_b] {
        let mut hp = 96;
        while hp > 24 {
            let ServerMessage::Event(event) = next(conn).await else {
                panic!("expected an event");
            };
            let CombatEvent::Damaged { hit_points, .. } = event.event else {
                panic!("expected damage, got {:?}", event.event);
            };
            assert_eq!(hit_points.current, hp - 3);
            hp = hit_points.current;
        }
    }
}

#[tokio::test]
async fn paused_combats_resume_days_later_on_any_instance() {
    let (ours, theirs) = (campaign(), campaign());
    let (combat, paused) = {
        let session = Server::start().await;
        let api = &session.client;
        let combat = api.create(&ours, "Bridge fight").await.unwrap().id;
        let a = add(api, combat, "A", 0).await;
        let b = add(api, combat, "B", 0).await;
        for (combatant, total) in [(a, 15), (b, 10)] {
            run(
                api,
                combat,
                CombatCommand::SetInitiative { combatant, total },
            )
            .await;
        }
        run(api, combat, CombatCommand::Start).await;
        run(api, combat, CombatCommand::NextTurn).await;
        run(api, combat, CombatCommand::Pause).await;
        (combat, api.snapshot(combat).await.unwrap())
    };
    assert_eq!(paused.state.status, CombatStatus::Paused);

    // Meanwhile another campaign fights on another instance.
    let other = Server::start().await;
    let elsewhere = other
        .client
        .create(&theirs, "Tavern brawl")
        .await
        .unwrap()
        .id;
    let x = add(&other.client, elsewhere, "X", 0).await;
    run(
        &other.client,
        elsewhere,
        CombatCommand::RollInitiativeForAll,
    )
    .await;
    run(&other.client, elsewhere, CombatCommand::Start).await;
    run(
        &other.client,
        elsewhere,
        CombatCommand::Damage {
            combatant: x,
            amount: 3,
        },
    )
    .await;

    // Days later, on a fresh instance: find the paused combat and carry on.
    let later = Server::start().await;
    let api = &later.client;
    let waiting = api
        .list(Some(&ours), Some(CombatStatus::Paused))
        .await
        .unwrap();
    assert_eq!(waiting.iter().map(|c| c.id).collect::<Vec<_>>(), [combat]);
    assert!(
        api.list(Some(&ours), Some(CombatStatus::Active))
            .await
            .unwrap()
            .is_empty()
    );
    let theirs_listed = api.list(Some(&theirs), None).await.unwrap();
    assert_eq!(
        theirs_listed.iter().map(|c| c.id).collect::<Vec<_>>(),
        [elsewhere]
    );

    assert_eq!(api.snapshot(combat).await.unwrap(), paused);
    let early = api
        .command(combat, &CommandEnvelope::new(CombatCommand::NextTurn))
        .await;
    assert!(
        matches!(&early, Err(campaign_client::ClientError::Rejected(e)) if e.contains("paused")),
        "{early:?}"
    );
    run(api, combat, CombatCommand::Resume).await;
    run(api, combat, CombatCommand::NextTurn).await;
    let state = api.snapshot(combat).await.unwrap().state;
    assert_eq!((state.status, state.round), (CombatStatus::Active, 2));
}

#[tokio::test]
async fn hit_points_are_written_back_to_character_sheets() {
    let characters = character_service().await;
    let sheet = campaign_types::CharacterSheet {
        name: "Vex".into(),
        hit_points: campaign_types::HitPoints {
            max: 30,
            current: 30,
            temporary: 0,
        },
        ..Default::default()
    };
    let vex = characters.create(&sheet).await.unwrap();
    let config = combat_service::Config {
        character_service: Some(characters.base_url().to_string()),
    };
    let a = Server::start_with(config.clone()).await;
    let b = Server::start_with(config).await;

    let combat = a.client.create(&campaign(), "Ambush").await.unwrap().id;
    let combatant = Combatant::from_character(&vex);
    let id = combatant.id;
    run(&a.client, combat, CombatCommand::AddCombatant { combatant }).await;
    run(
        &b.client,
        combat,
        CombatCommand::Damage {
            combatant: id,
            amount: 8,
        },
    )
    .await;
    wait_for_hp(&characters, vex.id, (22, 0)).await;

    run(
        &a.client,
        combat,
        CombatCommand::SetTemporaryHitPoints {
            combatant: id,
            amount: 5,
        },
    )
    .await;
    run(
        &b.client,
        combat,
        CombatCommand::Damage {
            combatant: id,
            amount: 7,
        },
    )
    .await;
    wait_for_hp(&characters, vex.id, (20, 0)).await;
    run(
        &a.client,
        combat,
        CombatCommand::Heal {
            combatant: id,
            amount: 4,
        },
    )
    .await;
    wait_for_hp(&characters, vex.id, (24, 0)).await;
}

async fn character_service() -> campaign_client::Client {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    character_service::migrate(&pool).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, character_service::app(pool)).into_future());
    campaign_client::Client::new(format!("http://{addr}"))
}

/// Write-back happens in the background, shortly after each command.
async fn wait_for_hp(characters: &campaign_client::Client, id: Uuid, expected: (u16, u16)) {
    let mut last = None;
    for _ in 0..50 {
        let hp = characters.get(id).await.unwrap().sheet.hit_points;
        if (hp.current, hp.temporary) == expected {
            return;
        }
        last = Some(hp);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("sheet never reached {expected:?}; last saw {last:?}");
}
