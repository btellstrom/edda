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
use campaign_types::combat::{
    ClientMessage, CombatSnapshot, CombatSummary, CommandEnvelope, LoggedEvent, ServerMessage,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

use crate::{
    error::ApiError,
    hub::{Hub, LiveCombat, Outcome},
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

async fn create(
    State(hub): State<AppState>,
    Json(body): Json<NewCombat>,
) -> Result<impl IntoResponse, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let snapshot = hub.create(&body.name).await?;
    let location = format!("/combats/{}", snapshot.id);
    Ok((
        StatusCode::CREATED,
        [(header::LOCATION, location)],
        Json(snapshot),
    ))
}

async fn list(State(hub): State<AppState>) -> Result<Json<Vec<CombatSummary>>, ApiError> {
    Ok(Json(hub.store().list().await?))
}

/// Current state: round, whose turn it is, initiative order and effects.
async fn snapshot(
    State(hub): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CombatSnapshot>, ApiError> {
    Ok(Json(hub.combat(id).await?.snapshot().await))
}

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default)]
    after: u64,
}

/// The complete log, or the part after `?after=<seq>`.
async fn log(
    State(hub): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<LoggedEvent>>, ApiError> {
    let events = hub.store().events(id, query.after).await?;
    Ok(Json(events.ok_or(ApiError::NotFound)?))
}

/// Runs a command and returns the events it produced. Resending a command
/// returns its original events rather than applying it twice.
async fn command(
    State(hub): State<AppState>,
    Path(id): Path<Uuid>,
    Json(envelope): Json<CommandEnvelope>,
) -> Result<Json<Vec<LoggedEvent>>, ApiError> {
    let command_id = envelope.command_id;
    match hub.execute(id, envelope).await? {
        Outcome::Applied(events) => Ok(Json(events)),
        Outcome::AlreadyApplied => Ok(Json(hub.store().events_for_command(id, command_id).await?)),
    }
}

async fn websocket(
    State(hub): State<AppState>,
    Path(id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    // Resolve the combat first so an unknown id is a plain 404.
    let combat = hub.combat(id).await?;
    Ok(upgrade.on_upgrade(move |socket| session(socket, hub, combat, id)))
}

/// One connected client: snapshot first, then every event as it happens,
/// while executing whatever commands the client sends.
async fn session(socket: WebSocket, hub: AppState, combat: Arc<LiveCombat>, id: Uuid) {
    let (mut sink, mut incoming) = socket.split();
    let (snapshot, mut events) = combat.subscribe().await;
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
                Err(RecvError::Lagged(_)) => {
                    let (snapshot, receiver) = combat.subscribe().await;
                    events = receiver;
                    ServerMessage::Snapshot(snapshot)
                }
                Err(RecvError::Closed) => break,
            },
            message = incoming.next() => match message {
                Some(Ok(Message::Text(text))) => match handle(&hub, id, &text).await {
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
async fn handle(hub: &Hub, id: Uuid, text: &str) -> Option<ServerMessage> {
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
    match hub.execute(id, envelope).await {
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
