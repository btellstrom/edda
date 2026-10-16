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
use campaign_auth::{protocol::Claims, server::Authenticated};
use campaign_types::combat::{
    ClientMessage, CombatSnapshot, CombatStatus, CombatSummary, CommandEnvelope, LoggedEvent,
    ServerMessage,
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
    name: String,
}

/// Creates a combat in the caller's campaign.
async fn create(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Json(body): Json<NewCombat>,
) -> Result<impl IntoResponse, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let snapshot = hub.create(&claims.campaign.to_string(), &body.name).await?;
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
    Ok(Json(combat.snapshot(hub.store()).await?))
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
    let events = hub.store().events(id, query.after).await?;
    Ok(Json(events.ok_or(ApiError::NotFound)?))
}

/// Runs a command and returns the events it produced. Resending a command
/// returns its original events rather than applying it twice.
async fn command(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    Json(envelope): Json<CommandEnvelope>,
) -> Result<Json<Vec<LoggedEvent>>, ApiError> {
    hub.combat_in(id, claims.campaign).await?;
    let envelope = attributed(envelope, &claims);
    let command_id = envelope.command_id;
    match hub.execute(id, envelope, &claims.key.to_string()).await? {
        Outcome::Applied(events) => Ok(Json(events)),
        Outcome::AlreadyApplied => Ok(Json(hub.store().events_for_command(id, command_id).await?)),
    }
}

async fn websocket(
    State(hub): State<AppState>,
    Authenticated(claims): Authenticated,
    Path(id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // Resolve the combat first so an unknown id is a plain 404.
    let combat = hub.combat_in(id, claims.campaign).await?;
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
    if send(&mut sink, &ServerMessage::Snapshot(snapshot))
        .await
        .is_err()
    {
        return;
    }
    loop {
        let reply = tokio::select! {
            event = events.recv() => match event {
                Ok(event) => ServerMessage::Event(event),
                // Too far behind to catch up event by event: start over
                // from a fresh snapshot.
                Err(RecvError::Lagged(_)) => match combat.subscribe(hub.store()).await {
                    Ok((snapshot, receiver)) => {
                        events = receiver;
                        ServerMessage::Snapshot(snapshot)
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
    match hub
        .execute(id, attributed(envelope, claims), &claims.key.to_string())
        .await
    {
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
