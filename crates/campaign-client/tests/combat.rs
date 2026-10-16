//! End-to-end tests against a real combat service.

use std::{path::PathBuf, time::Duration};

use campaign_client::combat::{CombatClient, Received, Replica, live::CombatConnection};
use campaign_types::combat::{
    CombatCommand, CombatEvent, CombatState, CombatStatus, Combatant, CommandEnvelope, NewEffect,
    ServerMessage,
};
use uuid::Uuid;

struct Server {
    client: CombatClient,
    db: PathBuf,
}

impl Server {
    async fn start() -> Self {
        let db = std::env::temp_dir().join(format!("combat-test-{}.db", Uuid::new_v4()));
        Self::start_on(db).await
    }

    /// Starts a server on an existing database, as after a restart.
    async fn start_on(db: PathBuf) -> Self {
        let pool = combat_service::connect(&format!("sqlite://{}", db.display()))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, combat_service::app(pool)).into_future());
        Self {
            client: CombatClient::new(format!("http://{addr}")),
            db,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.db.display()));
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
    let combat = api.create("Goblin ambush").await.unwrap().id;

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

    let combats = api.list().await.unwrap();
    assert_eq!(
        (combats[0].status, combats[0].round),
        (CombatStatus::Active, 1)
    );
}

#[tokio::test]
async fn changes_are_pushed_to_every_connected_client() {
    let server = Server::start().await;
    let api = &server.client;
    let combat = api.create("Live").await.unwrap().id;
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
    let combat = api.create("Retry").await.unwrap().id;
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
    let combat = api.create("Melee").await.unwrap().id;
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
async fn combats_survive_a_restart() {
    let db = std::env::temp_dir().join(format!("combat-test-{}.db", Uuid::new_v4()));
    let (combat, before) = {
        let server = Server::start_on(db.clone()).await;
        let api = &server.client;
        let combat = api.create("Persistent").await.unwrap().id;
        let a = add(api, combat, "A", 0).await;
        run(
            api,
            combat,
            CombatCommand::SetInitiative {
                combatant: a,
                total: 12,
            },
        )
        .await;
        run(api, combat, CombatCommand::Start).await;
        let before = api.snapshot(combat).await.unwrap();
        std::mem::forget(server); // keep the database file
        (combat, before)
    };

    let restarted = Server::start_on(db).await;
    let after = restarted.client.snapshot(combat).await.unwrap();
    assert_eq!(after, before);
    // And it carries on from where it was.
    run(&restarted.client, combat, CombatCommand::NextTurn).await;
    assert_eq!(
        restarted.client.snapshot(combat).await.unwrap().state.round,
        2
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
