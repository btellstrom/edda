//! Live combats on this instance: command execution and fan-out of events
//! to connected clients.
//!
//! Nothing here is the source of truth; the database is. Several instances
//! can serve the same combat at once:
//!
//! - Commands are serialised by locking the combat's row, and each instance
//!   catches its cached state up from the log under that lock before
//!   deciding anything.
//! - Every append is announced with a Postgres notification. Each instance
//!   listens, reads the new events from the log and pushes them to its own
//!   clients, so a command executed anywhere reaches everyone.
//!
//! A combat nobody is watching is dropped from memory after a while and
//! rebuilt from its log when next used, so combats can sit paused for days.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use campaign_types::combat::{
    CombatSnapshot, CombatState, CommandEnvelope, Dice, LoggedEvent, decide,
};
use sqlx::postgres::PgListener;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

use crate::{
    error::ApiError,
    store::{self, Notice, Store},
    sync::HpSync,
};

/// How many events a slow subscriber may fall behind before it is resynced
/// with a fresh snapshot instead.
const SUBSCRIBER_BUFFER: usize = 256;

/// How long a combat with no connected clients stays in memory.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub struct Hub {
    store: Store,
    live: Mutex<HashMap<Uuid, Arc<LiveCombat>>>,
    hp_sync: Option<Arc<HpSync>>,
}

pub struct LiveCombat {
    id: Uuid,
    /// The state commands are decided against. Held while a command runs.
    core: Mutex<View>,
    /// What this instance has broadcast so far. Subscribers start from a
    /// snapshot of it, taken under the same lock, so they miss nothing.
    feed: Mutex<View>,
    events: broadcast::Sender<LoggedEvent>,
    last_used: StdMutex<Instant>,
}

#[derive(Clone)]
struct View {
    state: CombatState,
    seq: u64,
}

impl View {
    fn apply(&mut self, event: &LoggedEvent) {
        self.state.apply(&event.event);
        self.seq = event.seq;
    }
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
    /// Starts listening for events from all instances, and the background
    /// upkeep: evicting idle combats and retrying HP write-back.
    pub async fn start(store: Store, hp_sync: Option<HpSync>) -> Result<Arc<Self>, sqlx::Error> {
        let mut listener = PgListener::connect_with(store.pool()).await?;
        listener.listen(store::CHANNEL).await?;
        let hub = Arc::new(Self {
            store,
            live: Mutex::new(HashMap::new()),
            hp_sync: hp_sync.map(Arc::new),
        });
        tokio::spawn(hub.clone().listen(listener));
        tokio::spawn(hub.clone().evict_idle());
        if let Some(sync) = &hub.hp_sync {
            tokio::spawn(sync.clone().retry_forever());
        }
        Ok(hub)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn create(&self, campaign: &str, name: &str) -> Result<CombatSnapshot, ApiError> {
        let id = self.store.create(campaign, name).await?;
        self.combat(id).await?.snapshot(&self.store).await
    }

    /// The live combat, loading it from its log if it isn't in memory.
    pub async fn combat(&self, id: Uuid) -> Result<Arc<LiveCombat>, ApiError> {
        // Held while loading, so a notification for this combat waits until
        // it is in the map rather than being missed.
        let mut live = self.live.lock().await;
        if let Some(combat) = live.get(&id) {
            combat.touch();
            return Ok(combat.clone());
        }
        let log = self.store.events(id, 0).await?.ok_or(ApiError::NotFound)?;
        let combat = Arc::new(LiveCombat::new(id, &log));
        live.insert(id, combat.clone());
        Ok(combat)
    }

    pub async fn execute(&self, id: Uuid, envelope: CommandEnvelope) -> Result<Outcome, ApiError> {
        let outcome = self
            .combat(id)
            .await?
            .execute(&self.store, envelope)
            .await?;
        if let (Outcome::Applied(_), Some(sync)) = (&outcome, &self.hp_sync) {
            tokio::spawn(sync.clone().run(id));
        }
        Ok(outcome)
    }

    async fn listen(self: Arc<Self>, mut listener: PgListener) {
        loop {
            match listener.try_recv().await {
                Ok(Some(notification)) => {
                    match serde_json::from_str::<Notice>(notification.payload()) {
                        Ok(notice) => {
                            let combat = self.live.lock().await.get(&notice.combat).cloned();
                            if let Some(combat) = combat {
                                let store = self.store.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = combat.catch_up(&store, Some(notice.seq)).await
                                    {
                                        tracing::error!(error = %e, "broadcasting events failed");
                                    }
                                });
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "ignoring malformed notification"),
                    }
                }
                // The connection dropped; the listener reconnects on the next
                // call, but notifications sent meanwhile are lost.
                Ok(None) => {
                    tracing::warn!("lost connection for notifications; catching up");
                    let combats: Vec<_> = self.live.lock().await.values().cloned().collect();
                    for combat in combats {
                        if let Err(e) = combat.catch_up(&self.store, None).await {
                            tracing::error!(error = %e, "catching up failed");
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "listening for notifications failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    async fn evict_idle(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            self.live
                .lock()
                .await
                .retain(|_, c| c.events.receiver_count() > 0 || c.idle_for() < IDLE_TIMEOUT);
        }
    }
}

impl LiveCombat {
    fn new(id: Uuid, log: &[LoggedEvent]) -> Self {
        let view = View {
            state: CombatState::replay(log.iter().map(|e| &e.event)),
            seq: log.last().map_or(0, |e| e.seq),
        };
        Self {
            id,
            core: Mutex::new(view.clone()),
            feed: Mutex::new(view),
            events: broadcast::channel(SUBSCRIBER_BUFFER).0,
            last_used: StdMutex::new(Instant::now()),
        }
    }

    fn touch(&self) {
        *self.last_used.lock().expect("not poisoned") = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used.lock().expect("not poisoned").elapsed()
    }

    /// The latest state, including events appended by other instances.
    pub async fn snapshot(&self, store: &Store) -> Result<CombatSnapshot, ApiError> {
        self.catch_up(store, None).await?;
        let feed = self.feed.lock().await;
        Ok(self.snapshot_of(&feed))
    }

    /// A snapshot plus a receiver for every event after it.
    pub async fn subscribe(
        &self,
        store: &Store,
    ) -> Result<(CombatSnapshot, broadcast::Receiver<LoggedEvent>), ApiError> {
        self.catch_up(store, None).await?;
        let feed = self.feed.lock().await;
        Ok((self.snapshot_of(&feed), self.events.subscribe()))
    }

    fn snapshot_of(&self, view: &View) -> CombatSnapshot {
        CombatSnapshot {
            id: self.id,
            seq: view.seq,
            state: view.state.clone(),
        }
    }

    /// Reads events past what has been broadcast and broadcasts them. With
    /// `Some(seq)`, does nothing if that position was already reached.
    async fn catch_up(&self, store: &Store, seq: Option<u64>) -> Result<(), ApiError> {
        let mut feed = self.feed.lock().await;
        if seq.is_some_and(|seq| seq <= feed.seq) {
            return Ok(());
        }
        for event in store.events(self.id, feed.seq).await?.unwrap_or_default() {
            feed.apply(&event);
            // No subscribers is fine; the log is the record.
            let _ = self.events.send(event);
        }
        Ok(())
    }

    async fn execute(&self, store: &Store, envelope: CommandEnvelope) -> Result<Outcome, ApiError> {
        let mut core = self.core.lock().await;
        let mut tx = store.begin().await?;
        let latest = store
            .lock(&mut tx, self.id)
            .await?
            .ok_or(ApiError::NotFound)?;
        if latest > core.seq {
            // Other instances have appended since; decide against the
            // state as it really is.
            for event in store.events_in(&mut tx, self.id, core.seq).await? {
                core.apply(&event);
            }
        }
        if !store
            .record_command(&mut tx, self.id, envelope.command_id)
            .await?
        {
            return Ok(Outcome::AlreadyApplied);
        }
        let events = decide(&core.state, &envelope.command, &mut ServerDice)?;

        let at = store::now();
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
        let mut next = core.clone();
        for event in &logged {
            next.apply(event);
        }
        // Clients hear about the events through the notification sent on
        // commit, the same way as about everyone else's.
        store.append(&mut tx, self.id, &logged, &next.state).await?;
        tx.commit().await?;
        *core = next;
        Ok(Outcome::Applied(logged))
    }
}
