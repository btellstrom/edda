//! Live combats: the authoritative in-memory state, command execution and
//! fan-out of events to subscribers.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use campaign_types::combat::{
    CombatSnapshot, CombatState, CommandEnvelope, Dice, LoggedEvent, decide,
};
use chrono::Utc;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

use crate::{error::ApiError, store::Store};

/// How many events a slow subscriber may fall behind before it is resynced
/// with a fresh snapshot instead.
const SUBSCRIBER_BUFFER: usize = 256;

pub struct Hub {
    store: Store,
    live: Mutex<HashMap<Uuid, Arc<LiveCombat>>>,
}

pub struct LiveCombat {
    id: Uuid,
    /// Held while a command is decided, persisted and broadcast, which is
    /// what makes this server the single authority on event order.
    core: Mutex<Core>,
    events: broadcast::Sender<LoggedEvent>,
}

struct Core {
    state: CombatState,
    seq: u64,
    /// Every command id applied so far, so resent commands are ignored.
    applied: HashSet<Uuid>,
}

pub enum Outcome {
    Applied(Vec<LoggedEvent>),
    AlreadyApplied,
}

struct ServerDice;

impl Dice for ServerDice {
    fn d20(&mut self) -> Option<u8> {
        Some(rand::random_range(1..=20))
    }
}

impl Hub {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            live: Mutex::new(HashMap::new()),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn create(&self, name: &str) -> Result<CombatSnapshot, ApiError> {
        let (id, created) = self.store.create(name).await?;
        let live = Arc::new(LiveCombat::new(id, vec![created]));
        let snapshot = live.snapshot().await;
        self.live.lock().await.insert(id, live);
        Ok(snapshot)
    }

    /// The live combat, loading it from its log on first use.
    pub async fn combat(&self, id: Uuid) -> Result<Arc<LiveCombat>, ApiError> {
        let mut live = self.live.lock().await;
        if let Some(combat) = live.get(&id) {
            return Ok(combat.clone());
        }
        let events = self.store.events(id, 0).await?.ok_or(ApiError::NotFound)?;
        let combat = Arc::new(LiveCombat::new(id, events));
        live.insert(id, combat.clone());
        Ok(combat)
    }

    pub async fn execute(&self, id: Uuid, envelope: CommandEnvelope) -> Result<Outcome, ApiError> {
        self.combat(id).await?.execute(&self.store, envelope).await
    }
}

impl LiveCombat {
    fn new(id: Uuid, log: Vec<LoggedEvent>) -> Self {
        let state = CombatState::replay(log.iter().map(|e| &e.event));
        let core = Core {
            state,
            seq: log.last().map_or(0, |e| e.seq),
            applied: log.iter().filter_map(|e| e.command_id).collect(),
        };
        Self {
            id,
            core: Mutex::new(core),
            events: broadcast::channel(SUBSCRIBER_BUFFER).0,
        }
    }

    pub async fn snapshot(&self) -> CombatSnapshot {
        let core = self.core.lock().await;
        CombatSnapshot {
            id: self.id,
            seq: core.seq,
            state: core.state.clone(),
        }
    }

    /// A snapshot plus a receiver for every event after it. Taken under the
    /// same lock as `execute`, so no event can fall between the two.
    pub async fn subscribe(&self) -> (CombatSnapshot, broadcast::Receiver<LoggedEvent>) {
        let core = self.core.lock().await;
        let receiver = self.events.subscribe();
        let snapshot = CombatSnapshot {
            id: self.id,
            seq: core.seq,
            state: core.state.clone(),
        };
        (snapshot, receiver)
    }

    async fn execute(&self, store: &Store, envelope: CommandEnvelope) -> Result<Outcome, ApiError> {
        let mut core = self.core.lock().await;
        if core.applied.contains(&envelope.command_id) {
            return Ok(Outcome::AlreadyApplied);
        }
        let events = decide(&core.state, &envelope.command, &mut ServerDice)?;

        let at = Utc::now();
        let logged: Vec<_> = events
            .into_iter()
            .zip(core.seq + 1..)
            .map(|(event, seq)| LoggedEvent {
                seq,
                command_id: Some(envelope.command_id),
                actor: envelope.actor.clone(),
                at,
                event,
            })
            .collect();

        // Persist before changing memory or telling anyone: if the write
        // fails, the command simply didn't happen.
        let mut next = core.state.clone();
        for e in &logged {
            next.apply(&e.event);
        }
        store.append(self.id, &logged, &next).await?;

        core.state = next;
        core.seq += logged.len() as u64;
        core.applied.insert(envelope.command_id);
        for e in &logged {
            // No subscribers is fine; the log is the record.
            let _ = self.events.send(e.clone());
        }
        Ok(Outcome::Applied(logged))
    }
}
