//! Client for the combat service: HTTP access, a live WebSocket connection
//! and [`Replica`], which gives frontends instant (predicted) feedback.

use edda_types::combat::{
    CombatError, CombatSnapshot, CombatState, CombatStatus, CombatSummary, CommandEnvelope,
    LoggedEvent, NoDice, ServerMessage, decide,
};
use edda_types::encounter::{Encounter, EncounterPlan};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::{ClientError, send};

/// The combat service's client: combats and planned encounters.
#[derive(Debug, Clone)]
pub struct CombatClient {
    base_url: String,
    http: reqwest::Client,
    token: Option<String>,
}

impl CombatClient {
    /// A client for the combat service at `base_url`, without a token.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: crate::http_client(None),
            token: None,
        }
    }

    /// Sends `token` (from [`crate::auth::AuthClient::login`]) with every
    /// request, and when connecting.
    pub fn with_token(mut self, token: &str) -> Self {
        self.http = crate::http_client(Some(token));
        self.token = Some(token.to_string());
        self
    }

    /// The logged-in campaign's combats, most recently active first,
    /// optionally only those in one status (e.g. paused combats waiting to
    /// be resumed).
    pub async fn list(
        &self,
        status: Option<CombatStatus>,
    ) -> Result<Vec<CombatSummary>, ClientError> {
        #[derive(Serialize)]
        struct Filter {
            #[serde(skip_serializing_if = "Option::is_none")]
            status: Option<CombatStatus>,
        }
        let request = self
            .http
            .get(self.url("/combats"))
            .query(&Filter { status });
        send(request).await
    }

    /// Creates a combat in the logged-in campaign.
    pub async fn create(&self, name: &str) -> Result<CombatSnapshot, ClientError> {
        let body = json!({ "name": name });
        send(self.http.post(self.url("/combats")).json(&body)).await
    }

    /// Creates a combat from a planned encounter, with the plan's monsters
    /// in it, called `name` or after the plan. All or nothing.
    pub async fn create_from_plan(
        &self,
        encounter: Uuid,
        name: Option<&str>,
    ) -> Result<CombatSnapshot, ClientError> {
        let body = json!({ "encounter": encounter, "name": name });
        send(self.http.post(self.url("/combats")).json(&body)).await
    }

    /// Round, whose turn it is, initiative order and effects in play.
    pub async fn snapshot(&self, id: Uuid) -> Result<CombatSnapshot, ClientError> {
        send(self.http.get(self.url(&format!("/combats/{id}")))).await
    }

    /// The log after position `after` (0 for all of it).
    pub async fn log(&self, id: Uuid, after: u64) -> Result<Vec<LoggedEvent>, ClientError> {
        let url = self.url(&format!("/combats/{id}/log?after={after}"));
        send(self.http.get(url)).await
    }

    /// Runs a command over plain HTTP and returns the events it produced.
    /// Safe to retry: a repeated `command_id` returns the original events.
    pub async fn command(
        &self,
        id: Uuid,
        envelope: &CommandEnvelope,
    ) -> Result<Vec<LoggedEvent>, ClientError> {
        let url = self.url(&format!("/combats/{id}/commands"));
        send(self.http.post(url).json(envelope)).await
    }

    /// The campaign's planned encounters, by name.
    pub async fn encounters(&self) -> Result<Vec<Encounter>, ClientError> {
        send(self.http.get(self.url("/encounters"))).await
    }

    /// One planned encounter.
    pub async fn encounter(&self, id: Uuid) -> Result<Encounter, ClientError> {
        send(self.http.get(self.url(&format!("/encounters/{id}")))).await
    }

    /// Keeps a new plan.
    pub async fn create_encounter(&self, plan: &EncounterPlan) -> Result<Encounter, ClientError> {
        send(self.http.post(self.url("/encounters")).json(plan)).await
    }

    /// Replaces a plan; the last save wins.
    pub async fn replace_encounter(
        &self,
        id: Uuid,
        plan: &EncounterPlan,
    ) -> Result<Encounter, ClientError> {
        send(
            self.http
                .put(self.url(&format!("/encounters/{id}")))
                .json(plan),
        )
        .await
    }

    /// Deletes a plan.
    pub async fn delete_encounter(&self, id: Uuid) -> Result<(), ClientError> {
        let response = self
            .http
            .delete(self.url(&format!("/encounters/{id}")))
            .send()
            .await?;
        crate::check(response).await.map(drop)
    }

    /// Opens a live connection. The first message is always a snapshot.
    #[cfg(feature = "websocket")]
    pub async fn connect(&self, id: Uuid) -> Result<live::CombatConnection, ClientError> {
        let url = self
            .url(&format!("/combats/{id}/ws"))
            .replacen("http", "ws", 1);
        live::CombatConnection::open(&url, self.token.as_deref()).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }
}

/// Following a combat live, over its WebSocket.
#[cfg(feature = "websocket")]
pub mod live {
    use edda_types::combat::{ClientMessage, CommandEnvelope, ServerMessage};
    use futures_util::{
        SinkExt, StreamExt,
        stream::{SplitSink, SplitStream},
    };
    use tokio::net::TcpStream;
    use tokio_tungstenite::{
        MaybeTlsStream, WebSocketStream,
        tungstenite::{self, Message, client::IntoClientRequest},
    };

    use crate::ClientError;

    type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    /// A live combat connection. Use [`CombatConnection::split`] to send and
    /// receive from different tasks.
    pub struct CombatConnection {
        sender: CommandSender,
        receiver: MessageReceiver,
    }

    /// The half of a [`CombatConnection`] that sends commands.
    pub struct CommandSender(SplitSink<Socket, Message>);

    /// The half of a [`CombatConnection`] that receives what the server sends.
    pub struct MessageReceiver(SplitStream<Socket>);

    impl CombatConnection {
        pub(super) async fn open(url: &str, token: Option<&str>) -> Result<Self, ClientError> {
            let ws_error = |e: tungstenite::Error| ClientError::WebSocket(e.to_string());
            let mut request = url.into_client_request().map_err(ws_error)?;
            if let Some(token) = token {
                let value = format!("Bearer {token}")
                    .parse()
                    .expect("tokens are plain ASCII");
                request.headers_mut().insert("authorization", value);
            }
            let (socket, _) = tokio_tungstenite::connect_async(request)
                .await
                .map_err(ws_error)?;
            let (sink, stream) = socket.split();
            Ok(Self {
                sender: CommandSender(sink),
                receiver: MessageReceiver(stream),
            })
        }

        /// Sends a command; its events come back like everyone else's.
        pub async fn send(&mut self, envelope: &CommandEnvelope) -> Result<(), ClientError> {
            self.sender.send(envelope).await
        }

        /// The next message from the server; `None` once the connection has
        /// closed.
        pub async fn recv(&mut self) -> Option<Result<ServerMessage, ClientError>> {
            self.receiver.recv().await
        }

        /// Splits the connection, to send and receive from different tasks.
        pub fn split(self) -> (CommandSender, MessageReceiver) {
            (self.sender, self.receiver)
        }
    }

    impl CommandSender {
        /// Sends a command; its events come back like everyone else's.
        pub async fn send(&mut self, envelope: &CommandEnvelope) -> Result<(), ClientError> {
            let message = ClientMessage::Command(envelope.clone());
            let text = serde_json::to_string(&message).expect("commands always serialize");
            self.0
                .send(Message::Text(text.into()))
                .await
                .map_err(|e| ClientError::WebSocket(e.to_string()))
        }
    }

    impl MessageReceiver {
        /// The next message from the server; `None` once the connection is
        /// closed.
        pub async fn recv(&mut self) -> Option<Result<ServerMessage, ClientError>> {
            loop {
                return match self.0.next().await? {
                    Ok(Message::Text(text)) => Some(
                        serde_json::from_str(&text)
                            .map_err(|e| ClientError::WebSocket(format!("bad message: {e}"))),
                    ),
                    Ok(Message::Close(_)) => None,
                    Ok(_) => continue,
                    Err(e) => Some(Err(ClientError::WebSocket(e.to_string()))),
                };
            }
        }
    }
}

/// A client's local copy of a combat, for optimistic UIs.
///
/// Holds the state the server has confirmed plus this client's commands
/// that are still in flight. [`Replica::state`] is the confirmed state with
/// the pending commands applied on top, so a user sees the result of their
/// action immediately. When the server's events arrive (in its order, which
/// may put other players' actions first) the pending commands are replayed
/// on top of the new confirmed state; if the server rejects one, it simply
/// disappears from the prediction.
///
/// Contains no IO, so any frontend (native or WASM) can drive it from
/// whatever transport it has.
#[derive(Debug, Clone)]
pub struct Replica {
    confirmed: CombatState,
    seq: u64,
    pending: Vec<CommandEnvelope>,
    predicted: CombatState,
}

/// What a message from the server did to a [`Replica`].
#[derive(Debug, Clone, PartialEq)]
pub enum Received {
    /// The state may have changed; re-render.
    Updated,
    /// One of this client's commands was refused. The prediction has
    /// already been rolled back.
    Rejected {
        envelope: CommandEnvelope,
        error: String,
    },
    /// Events were missed (e.g. after a network hiccup). Reconnect, or
    /// fetch a snapshot and pass it to [`Replica::receive`], then resend
    /// [`Replica::pending`].
    NeedsResync,
}

impl Replica {
    /// A replica starting from `snapshot`, with nothing pending.
    pub fn new(snapshot: CombatSnapshot) -> Self {
        Self {
            predicted: snapshot.state.clone(),
            confirmed: snapshot.state,
            seq: snapshot.seq,
            pending: Vec::new(),
        }
    }

    /// What to show: the confirmed state plus this client's pending
    /// commands.
    pub fn state(&self) -> &CombatState {
        &self.predicted
    }

    /// What the server has confirmed.
    pub fn confirmed(&self) -> &CombatState {
        &self.confirmed
    }

    /// The last event the confirmed state includes.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Commands sent but not yet confirmed or rejected.
    pub fn pending(&self) -> &[CommandEnvelope] {
        &self.pending
    }

    /// Records a command about to be sent and applies its predicted outcome.
    /// Commands that are certainly invalid fail here, instantly, and should
    /// not be sent. Commands whose outcome needs a server dice roll, and
    /// undos, are accepted but only show up once the server answers.
    pub fn submit(&mut self, envelope: CommandEnvelope) -> Result<(), CombatError> {
        match decide(&self.predicted, &envelope.command, &mut NoDice) {
            Ok(_) | Err(CombatError::DiceUnavailable | CombatError::ServerOnly) => {}
            Err(e) => return Err(e),
        }
        self.pending.push(envelope);
        self.rebuild();
        Ok(())
    }

    /// Takes in a message from the server, replaying pending commands on top
    /// of what it confirms.
    pub fn receive(&mut self, message: ServerMessage) -> Received {
        let outcome = match message {
            ServerMessage::Snapshot(snapshot) => {
                self.confirmed = snapshot.state;
                self.seq = snapshot.seq;
                Received::Updated
            }
            ServerMessage::Event(event) => {
                if event.seq <= self.seq {
                    return Received::Updated; // Already have it.
                }
                if event.seq != self.seq + 1 {
                    return Received::NeedsResync;
                }
                self.confirmed.apply(&event.event);
                self.seq = event.seq;
                if let Some(id) = event.command_id {
                    self.pending.retain(|p| p.command_id != id);
                }
                Received::Updated
            }
            ServerMessage::Rejected { command_id, error } => {
                match self.pending.iter().position(|p| p.command_id == command_id) {
                    Some(i) => Received::Rejected {
                        envelope: self.pending.remove(i),
                        error,
                    },
                    None => Received::Updated,
                }
            }
            ServerMessage::AlreadyApplied { command_id } => {
                self.pending.retain(|p| p.command_id != command_id);
                Received::Updated
            }
        };
        self.rebuild();
        outcome
    }

    fn rebuild(&mut self) {
        let mut state = self.confirmed.clone();
        for envelope in &self.pending {
            // A pending command that no longer applies (someone else got
            // there first) or needs dice just isn't shown; the server will
            // have the final word.
            if let Ok(events) = decide(&state, &envelope.command, &mut NoDice) {
                for event in &events {
                    state.apply(event);
                }
            }
        }
        self.predicted = state;
    }
}

#[cfg(test)]
mod tests {
    use edda_types::combat::{CombatCommand, CombatEvent, Combatant, LoggedEvent};

    use super::*;

    fn event(seq: u64, command_id: Option<Uuid>, event: CombatEvent) -> ServerMessage {
        ServerMessage::Event(LoggedEvent {
            seq,
            command_id,
            actor: None,
            at: Default::default(),
            event,
        })
    }

    /// A replica of a combat containing one goblin, plus the goblin's id.
    fn replica() -> (Replica, Uuid) {
        let goblin = Combatant::new("Goblin", 7);
        let id = goblin.id;
        let mut state = CombatState::default();
        state.apply(&CombatEvent::Created {
            campaign: "Test".into(),
            name: "Ambush".into(),
        });
        state.apply(&CombatEvent::CombatantAdded { combatant: goblin });
        let snapshot = CombatSnapshot {
            id: Uuid::new_v4(),
            seq: 2,
            state,
        };
        (Replica::new(snapshot), id)
    }

    fn hp(r: &CombatState, id: Uuid) -> u16 {
        r.combatant(id).unwrap().hit_points.current
    }

    /// The events the server would produce for `command` against the
    /// replica's confirmed state.
    fn server_events(r: &Replica, command: &CombatCommand) -> Vec<CombatEvent> {
        decide(r.confirmed(), command, &mut NoDice).unwrap()
    }

    #[test]
    fn own_commands_show_immediately_and_are_confirmed() {
        let (mut r, goblin) = replica();
        let hit = CommandEnvelope::new(CombatCommand::Damage {
            combatant: goblin,
            amount: 3,
        });
        r.submit(hit.clone()).unwrap();
        assert_eq!(hp(r.state(), goblin), 4, "predicted");
        assert_eq!(hp(r.confirmed(), goblin), 7, "not yet confirmed");

        let [e] = server_events(&r, &hit.command).try_into().unwrap();
        assert_eq!(
            r.receive(event(3, Some(hit.command_id), e)),
            Received::Updated
        );
        assert!(r.pending().is_empty());
        assert_eq!(hp(r.state(), goblin), 4);
        assert_eq!(r.state(), r.confirmed());
    }

    #[test]
    fn pending_commands_are_replayed_after_other_players_actions() {
        let (mut r, goblin) = replica();
        // We hit for 3...
        let ours = CommandEnvelope::new(CombatCommand::Damage {
            combatant: goblin,
            amount: 3,
        });
        r.submit(ours.clone()).unwrap();
        // ...but the server processes another player's 2 damage first.
        let theirs = CombatCommand::Damage {
            combatant: goblin,
            amount: 2,
        };
        let [e] = server_events(&r, &theirs).try_into().unwrap();
        r.receive(event(3, Some(Uuid::new_v4()), e));
        assert_eq!(hp(r.confirmed(), goblin), 5);
        assert_eq!(hp(r.state(), goblin), 2, "ours rebased on top of theirs");
    }

    #[test]
    fn rejected_commands_roll_back() {
        let (mut r, goblin) = replica();
        let remove = CommandEnvelope::new(CombatCommand::RemoveCombatant { combatant: goblin });
        r.submit(remove.clone()).unwrap();
        assert!(r.state().combatants.is_empty());

        let outcome = r.receive(ServerMessage::Rejected {
            command_id: remove.command_id,
            error: "nope".into(),
        });
        assert_eq!(
            outcome,
            Received::Rejected {
                envelope: remove,
                error: "nope".into()
            }
        );
        assert_eq!(r.state().combatants.len(), 1);
    }

    #[test]
    fn certainly_invalid_commands_fail_locally() {
        let (mut r, _) = replica();
        let bad = CombatCommand::Heal {
            combatant: Uuid::new_v4(),
            amount: 1,
        };
        assert!(matches!(
            r.submit(CommandEnvelope::new(bad)),
            Err(CombatError::UnknownCombatant(_))
        ));
        assert!(r.pending().is_empty());
    }

    #[test]
    fn dice_rolls_wait_for_the_server() {
        let (mut r, goblin) = replica();
        let roll = CommandEnvelope::new(CombatCommand::RollInitiative {
            combatant: goblin,
            roll: None,
        });
        r.submit(roll.clone()).unwrap();
        assert_eq!(r.pending().len(), 1);
        assert_eq!(r.state().combatant(goblin).unwrap().initiative, None);

        let rolled = CombatEvent::InitiativeRolled {
            combatant: goblin,
            roll: 14,
            modifier: 0,
            total: 14,
        };
        r.receive(event(3, Some(roll.command_id), rolled));
        assert_eq!(r.state().combatant(goblin).unwrap().initiative, Some(14));
        assert!(r.pending().is_empty());
    }

    #[test]
    fn gaps_and_duplicates_are_detected() {
        let (mut r, _) = replica();
        assert_eq!(
            r.receive(event(2, None, CombatEvent::Started)),
            Received::Updated,
            "old"
        );
        assert_eq!(r.state().status, Default::default(), "old event ignored");
        assert_eq!(
            r.receive(event(5, None, CombatEvent::Started)),
            Received::NeedsResync
        );
    }

    #[test]
    fn already_applied_commands_are_dropped() {
        let (mut r, goblin) = replica();
        let hit = CommandEnvelope::new(CombatCommand::Damage {
            combatant: goblin,
            amount: 3,
        });
        r.submit(hit.clone()).unwrap();
        // After a reconnect the snapshot already includes the hit...
        let mut state = r.confirmed().clone();
        for e in server_events(&r, &hit.command) {
            state.apply(&e);
        }
        r.receive(ServerMessage::Snapshot(CombatSnapshot {
            id: Uuid::nil(),
            seq: 3,
            state,
        }));
        assert_eq!(
            hp(r.state(), goblin),
            1,
            "briefly double-counted until the server says so"
        );
        r.receive(ServerMessage::AlreadyApplied {
            command_id: hit.command_id,
        });
        assert_eq!(hp(r.state(), goblin), 4);
    }
}
