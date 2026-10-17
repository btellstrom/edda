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
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};

use campaign_auth::protocol::{Claims, Principal};
use campaign_types::{
    combat::{
        CombatCommand, CombatEvent, CombatSnapshot, CombatState, CommandEnvelope, Dice,
        LoggedEvent, SheetCost, add_monsters, decide, player_may, undo,
    },
    monster::Monster,
};
use sqlx::postgres::PgListener;
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

use crate::{
    encounters,
    error::ApiError,
    services::Monsters,
    store::{self, Notice, Store},
    sync::SheetSync,
};

/// How many events a slow subscriber may fall behind before it is resynced
/// with a fresh snapshot instead.
const SUBSCRIBER_BUFFER: usize = 256;

/// How long a combat with no connected clients stays in memory.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

pub struct Hub {
    store: Store,
    live: Mutex<HashMap<Uuid, Arc<LiveCombat>>>,
    sheet_sync: Option<Arc<SheetSync>>,
    monsters: Option<Monsters>,
}

pub struct LiveCombat {
    id: Uuid,
    /// The campaign it belongs to; fixed at creation.
    campaign: String,
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
    fn roll(&mut self, sides: u16) -> Option<u16> {
        Some(rand::random_range(1..=sides.max(1)))
    }
}

impl Hub {
    /// Starts listening for events from all instances, and the background
    /// upkeep: evicting idle combats and retrying HP write-back. Monsters
    /// are added from `monsters`.
    pub async fn start(
        store: Store,
        sheet_sync: Option<SheetSync>,
        monsters: Option<Monsters>,
    ) -> Result<Arc<Self>, sqlx::Error> {
        let mut listener = PgListener::connect_with(store.pool()).await?;
        listener.listen(store::CHANNEL).await?;
        let hub = Arc::new(Self {
            store,
            live: Mutex::new(HashMap::new()),
            sheet_sync: sheet_sync.map(Arc::new),
            monsters,
        });
        tokio::spawn(hub.clone().listen(listener));
        tokio::spawn(hub.clone().evict_idle());
        if let Some(sync) = &hub.sheet_sync {
            tokio::spawn(sync.clone().retry_forever());
        }
        Ok(hub)
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub async fn create(&self, campaign: &str, name: &str) -> Result<CombatSnapshot, ApiError> {
        let id = self.store.create(campaign, name, Vec::new()).await?;
        self.combat(id).await?.snapshot(&self.store).await
    }

    /// Creates a combat from one of `campaign`'s plans, with the plan's
    /// monsters in it, and called `name` or after the plan. All or nothing:
    /// if any of the monsters can't be added, there's no combat.
    pub async fn create_from_plan(
        &self,
        campaign: Uuid,
        encounter: Uuid,
        name: Option<&str>,
    ) -> Result<CombatSnapshot, ApiError> {
        let plan = encounters::plan(self, campaign, encounter).await?;
        let name = name.filter(|n| !n.trim().is_empty()).unwrap_or(&plan.name);
        let mut state = CombatState::default();
        state.apply(&CombatEvent::Created {
            name: name.to_string(),
            campaign: campaign.to_string(),
        });
        let mut setup = Vec::new();
        for entry in &plan.monsters {
            let monster = match self.stat_block(campaign, entry.monster).await {
                Err(ApiError::BadRequest(_)) => {
                    return Err(ApiError::BadRequest(format!(
                        "{} from the plan isn't among the campaign's monsters any more",
                        entry.name
                    )));
                }
                other => other?,
            };
            for event in add_monsters(&state, &monster, entry.count, entry.roll, &mut ServerDice)? {
                state.apply(&event);
                setup.push(event);
            }
        }
        let id = self
            .store
            .create(&campaign.to_string(), name, setup)
            .await?;
        self.combat(id).await?.snapshot(&self.store).await
    }

    /// The characters a player's device plays, which are its combatants.
    async fn played_by(
        &self,
        combat: &LiveCombat,
        device: &str,
    ) -> Result<HashSet<Uuid>, ApiError> {
        let sync = self.sheet_sync.as_ref().ok_or_else(|| {
            ApiError::Forbidden(
                "players can't act here: this combat service can't reach the character sheets \
                 to know whose they are"
                    .into(),
            )
        })?;
        sync.played_by(combat.campaign_id()?, device)
            .await
            .map_err(|e| ApiError::SheetUnavailable(e.to_string()))
    }

    /// A monster's stat block, from the monster service.
    async fn stat_block(&self, campaign: Uuid, monster: Uuid) -> Result<Monster, ApiError> {
        let monsters = self.monsters.as_ref().ok_or_else(|| {
            ApiError::BadRequest(
                "this combat service can't add monsters (no monster service)".into(),
            )
        })?;
        monsters
            .get(campaign, monster)
            .await
            .map_err(ApiError::from_monsters)
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

    /// The combat, if it belongs to `campaign`. Another campaign's combat
    /// is reported as not found, so its existence isn't revealed.
    pub async fn combat_in(&self, id: Uuid, campaign: Uuid) -> Result<Arc<LiveCombat>, ApiError> {
        let combat = self.combat(id).await?;
        if combat.campaign != campaign.to_string() {
            return Err(ApiError::NotFound);
        }
        Ok(combat)
    }

    /// Runs a command sent by the device `claims` are for. A player's
    /// device may only change its own combatants (see [`player_may`]).
    /// What the command costs a character's sheet is spent first (no slot
    /// left, no cast), and given back if the command is then refused.
    pub async fn execute(
        &self,
        id: Uuid,
        envelope: CommandEnvelope,
        claims: &Claims,
    ) -> Result<Outcome, ApiError> {
        let combat = self.combat(id).await?;
        let command_id = envelope.command_id;
        let actor_key = claims.key.to_string();
        // Checked before anything is spent, and again under the combat's
        // lock against the latest state.
        let played = match claims.role {
            Principal::Player => {
                let played = self.played_by(&combat, &actor_key).await?;
                let state = combat.snapshot(&self.store).await?.state;
                permit(&state, &envelope.command, Some(&played))?;
                Some(played)
            }
            Principal::Dm | Principal::Service => None,
        };
        let stat_block = match &envelope.command {
            CombatCommand::AddMonsters { monster, .. } => {
                Some(self.stat_block(combat.campaign_id()?, *monster).await?)
            }
            _ => None,
        };
        let spent = self.spend(&combat, &envelope).await?;
        let outcome = combat
            .execute(
                &self.store,
                envelope,
                &actor_key,
                played.as_ref(),
                stat_block.as_ref(),
            )
            .await;
        if let (Err(_), Some((sync, campaign, character, cost))) = (&outcome, &spent) {
            let key = crate::sync::refused_key(command_id);
            if let Err(e) = sync.give_back(*campaign, *character, cost, key).await {
                tracing::error!(%id, %character, error = %e, "giving back what a refused command spent failed");
            }
        }
        let outcome = outcome?;
        if let (Outcome::Applied(events), Some(sync)) = (&outcome, &self.sheet_sync) {
            let undo = events
                .iter()
                .any(|e| matches!(e.event, CombatEvent::Undone { .. }));
            if undo {
                // What the undone command spent is back on the sheet before
                // anyone is told it was undone.
                sync.clone().run(id).await;
            } else {
                tokio::spawn(sync.clone().run(id));
            }
        }
        Ok(outcome)
    }

    /// Spends what `envelope`'s command costs a character's sheet, if
    /// anything; what to give back if the command is then refused.
    async fn spend(
        &self,
        combat: &LiveCombat,
        envelope: &CommandEnvelope,
    ) -> Result<Option<(Arc<SheetSync>, Uuid, Uuid, SheetCost)>, ApiError> {
        let (Some(sync), Some((combatant, cost))) =
            (&self.sheet_sync, envelope.command.sheet_cost())
        else {
            return Ok(None);
        };
        let snapshot = combat.snapshot(&self.store).await?;
        let Some(character) = snapshot
            .state
            .combatant(combatant)
            .and_then(|c| c.character_id)
        else {
            return Ok(None);
        };
        let campaign = combat.campaign_id()?;
        sync.spend(campaign, character, &cost, envelope.command_id)
            .await
            .map_err(ApiError::from_sheet)?;
        Ok(Some((sync.clone(), campaign, character, cost)))
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
            campaign: view.state.campaign.clone(),
            core: Mutex::new(view.clone()),
            feed: Mutex::new(view),
            events: broadcast::channel(SUBSCRIBER_BUFFER).0,
            last_used: StdMutex::new(Instant::now()),
        }
    }

    fn campaign_id(&self) -> Result<Uuid, ApiError> {
        self.campaign
            .parse()
            .map_err(|e: uuid::Error| ApiError::Corrupt(e.to_string()))
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
        // Gone from the log means deleted (with its campaign), whatever this
        // instance still has in memory.
        let events = store
            .events(self.id, feed.seq)
            .await?
            .ok_or(ApiError::NotFound)?;
        for event in events {
            feed.apply(&event);
            // No subscribers is fine; the log is the record.
            let _ = self.events.send(event);
        }
        Ok(())
    }

    /// Runs a command. For a player's device, `played` are the characters
    /// it plays; `AddMonsters` needs the monster's `stat_block`.
    async fn execute(
        &self,
        store: &Store,
        envelope: CommandEnvelope,
        actor_key: &str,
        played: Option<&HashSet<Uuid>>,
        stat_block: Option<&Monster>,
    ) -> Result<Outcome, ApiError> {
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
        permit(&core.state, &envelope.command, played)?;
        if !store
            .record_command(&mut tx, self.id, envelope.command_id, actor_key)
            .await?
        {
            return Ok(Outcome::AlreadyApplied);
        }
        let events = match envelope.command {
            CombatCommand::Undo { command } => {
                let log = store.events_in(&mut tx, self.id, 0).await?;
                let mine = store.commands_by(&mut tx, self.id, actor_key).await?;
                undo::undo(&log, command, |c| mine.contains(&c))?
            }
            CombatCommand::AddMonsters { count, roll, .. } => {
                let monster = stat_block.expect("fetched by Hub::execute");
                add_monsters(&core.state, monster, count, roll, &mut ServerDice)?
            }
            ref command => decide(&core.state, command, &mut ServerDice)?,
        };

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

/// Whether the device may send `command`: anything for a DM device or a
/// service (`played` is `None`), only what [`player_may`] allows for a
/// player's device, which plays the characters `played`.
fn permit(
    state: &CombatState,
    command: &CombatCommand,
    played: Option<&HashSet<Uuid>>,
) -> Result<(), ApiError> {
    let Some(played) = played else {
        return Ok(());
    };
    player_may(state, command, |c| {
        c.character_id.is_some_and(|id| played.contains(&id))
    })
    .map_err(|e| ApiError::Forbidden(e.to_string()))
}
