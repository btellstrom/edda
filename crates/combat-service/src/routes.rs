use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use edda_auth::{
    protocol::{Claims, Principal},
    server::Authenticated,
};
use edda_types::combat::{
    ClientMessage, CombatSnapshot, CombatState, CombatStatus, CombatSummary, CommandEnvelope,
    Concealer, LoggedEvent, ServerMessage,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::{
    error::ApiError,
    hub::{Hub, LiveCombat, Outcome},
    store::ListFilter,
};

type AppState = Arc<Hub>;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/combats", get(list).post(create))
        .route("/combats/{id}", get(snapshot))
        .route("/combats/{id}/log", get(log))
        .route("/combats/{id}/commands", post(command))
        .route("/combats/{id}/ws", get(websocket))
}

#[derive(Deserialize)]
struct NewCombat {
    /// What to call it; with a plan, the plan's name by default.
    #[serde(default)]
    name: Option<String>,
    /// A planned encounter to start it from, with the plan's monsters in it.
    #[serde(default)]
    encounter: Option<Uuid>,
}

/// Creates a combat in the caller's campaign (DM devices only), empty or
/// from a plan.
async fn create(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Json(body): Json<NewCombat>,
) -> Result<impl IntoResponse, ApiError> {
    if !claims.is_dm() {
        return Err(ApiError::Forbidden(
            "only a DM device can start a combat".into(),
        ));
    }
    let snapshot = match (body.encounter, body.name) {
        (Some(encounter), name) => {
            hub.create_from_plan(claims.campaign, encounter, name.as_deref())
                .await?
        }
        (None, Some(name)) if !name.trim().is_empty() => {
            hub.create(&claims.campaign.to_string(), &name).await?
        }
        (None, _) => return Err(ApiError::BadRequest("name must not be empty".into())),
    };
    let location = format!("/combats/{}", snapshot.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(snapshot),
    ))
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<CombatStatus>,
}

/// The caller's campaign's combats, most recently active first; filter with
/// `?status=paused`.
async fn list(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<CombatSummary>>, ApiError> {
    let filter = ListFilter {
        campaign: Some(claims.campaign.to_string()),
        status: query.status,
    };
    Ok(Json(hub.store().list(&filter).await?))
}

/// Current state: round, whose turn it is, initiative order and effects.
async fn snapshot(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<Json<CombatSnapshot>, ApiError> {
    let combat = hub.combat_in(id, claims.campaign).await?;
    let mut snapshot = combat.snapshot(hub.store()).await?;
    if behind_the_screen(&claims) {
        snapshot.state = Concealer::new(&snapshot.state).state(&snapshot.state);
    }
    Ok(Json(snapshot))
}

/// Whether the device sees monsters only as the players do (see
/// [`edda_types::combat::view`]).
fn behind_the_screen(claims: &Claims) -> bool {
    claims.role == Principal::Player
}

/// `events`, which follow the combat as it stands in `state`, as the device
/// sees them.
fn seen(claims: &Claims, state: &CombatState, events: Vec<LoggedEvent>) -> Vec<LoggedEvent> {
    if !behind_the_screen(claims) {
        return events;
    }
    let mut concealer = Concealer::new(state);
    events.iter().map(|e| concealer.event(e)).collect()
}

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default)]
    after: u64,
}

/// The complete log, or the part after `?after=<seq>`.
async fn log(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<LoggedEvent>>, ApiError> {
    hub.combat_in(id, claims.campaign).await?;
    if !behind_the_screen(&claims) {
        let events = hub.store().events(id, query.after).await?;
        return Ok(Json(events.ok_or(ApiError::NotFound)?));
    }
    // Concealing needs to know everyone from the start.
    let log = hub.store().events(id, 0).await?.ok_or(ApiError::NotFound)?;
    let mut seen = seen(&claims, &CombatState::default(), log);
    seen.retain(|e| e.seq > query.after);
    Ok(Json(seen))
}

/// Runs a command and returns the events it produced. Resending a command
/// returns its original events rather than applying it twice.
async fn command(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(envelope): Json<CommandEnvelope>,
) -> Result<Json<Vec<LoggedEvent>>, ApiError> {
    let combat = hub.combat_in(id, claims.campaign).await?;
    let envelope = attributed(envelope, &claims);
    let command_id = envelope.command_id;
    let events = match hub.execute(id, envelope, &claims).await? {
        Outcome::Applied(events) => events,
        Outcome::AlreadyApplied => hub.store().events_for_command(id, command_id).await?,
    };
    let state = combat.snapshot(hub.store()).await?.state;
    Ok(Json(seen(&claims, &state, events)))
}

async fn websocket(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // Resolve the combat first so an unknown id is a plain 404.
    let combat = hub.combat_in(id, claims.campaign).await?;
    let watching = i64::try_from(combat.watchers()).unwrap_or(i64::MAX);
    let limits = hub.limits();
    limits
        .room_for(
            watching,
            1,
            limits.connections_per_combat,
            "devices watching a combat",
        )
        .map_err(ApiError::Forbidden)?;
    Ok(upgrade.on_upgrade(move |socket| session(socket, hub, combat, id, claims)))
}

/// The log records who sent a command as the authenticated device's name,
/// whatever the client claimed.
fn attributed(mut envelope: CommandEnvelope, claims: &Claims) -> CommandEnvelope {
    envelope.actor = Some(claims.name.clone());
    envelope
}

/// One connected client: snapshot first, then every event as it happens,
/// while executing whatever commands the client sends.
async fn session(
    socket: WebSocket,
    hub: AppState,
    combat: Arc<LiveCombat>,
    id: Uuid,
    claims: Claims,
) {
    let (mut sink, mut incoming) = socket.split();
    let Ok((snapshot, mut events)) = combat.subscribe(hub.store()).await else {
        return;
    };
    // A player's device sees monsters as the players do.
    let mut view = View::new(&claims);
    if send(&mut sink, &ServerMessage::Snapshot(view.snapshot(snapshot)))
        .await
        .is_err()
    {
        return;
    }
    loop {
        let reply = tokio::select! {
            event = events.recv() => match event {
                Ok(event) => ServerMessage::Event(view.event(&event)),
                // Too far behind to catch up event by event: start over
                // from a fresh snapshot.
                Err(RecvError::Lagged(_)) => match combat.subscribe(hub.store()).await {
                    Ok((snapshot, receiver)) => {
                        events = receiver;
                        ServerMessage::Snapshot(view.snapshot(snapshot))
                    }
                    Err(_) => break,
                },
                Err(RecvError::Closed) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => match handle(&hub, id, &text, &claims).await {
                    Some(reply) => reply,
                    None => continue,
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => continue,
            },
        };
        if send(&mut sink, &reply).await.is_err() {
            break;
        }
    }
}

/// What one connected device sees: everything, or for a player's device
/// monsters concealed.
struct View {
    concealer: Option<Concealer>,
    player: bool,
}

impl View {
    fn new(claims: &Claims) -> Self {
        Self {
            concealer: None,
            player: behind_the_screen(claims),
        }
    }

    /// Starts over from `snapshot`.
    fn snapshot(&mut self, mut snapshot: CombatSnapshot) -> CombatSnapshot {
        if self.player {
            let concealer = Concealer::new(&snapshot.state);
            snapshot.state = concealer.state(&snapshot.state);
            self.concealer = Some(concealer);
        }
        snapshot
    }

    fn event(&mut self, event: &LoggedEvent) -> LoggedEvent {
        match &mut self.concealer {
            Some(concealer) => concealer.event(event),
            None => event.clone(),
        }
    }
}

/// Executes a client's command. Accepted commands need no direct reply:
/// their events reach this client through the broadcast like everyone else.
async fn handle(hub: &Hub, id: Uuid, text: &str, claims: &Claims) -> Option<ServerMessage> {
    let ClientMessage::Command(envelope) = match serde_json::from_str(text) {
        Ok(message) => message,
        Err(e) => {
            return Some(ServerMessage::Rejected {
                command_id: Uuid::nil(),
                error: format!("invalid message: {e}"),
            });
        }
    };
    let command_id = envelope.command_id;
    match hub.execute(id, attributed(envelope, claims), claims).await {
        Ok(Outcome::Applied(_)) => None,
        Ok(Outcome::AlreadyApplied) => Some(ServerMessage::AlreadyApplied { command_id }),
        Err(e) => Some(ServerMessage::Rejected {
            command_id,
            error: e.to_string(),
        }),
    }
}

async fn send(
    sink: &mut (impl SinkExt<Message, Error = axum::Error> + Unpin),
    message: &ServerMessage,
) -> Result<(), axum::Error> {
    let text = serde_json::to_string(message).expect("server messages always serialize");
    sink.send(Message::Text(text.into())).await
}
