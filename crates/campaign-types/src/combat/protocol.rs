//! Messages exchanged with the combat service.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{CombatCommand, CombatEvent, CombatState, CombatStatus};

/// A command plus the metadata that makes it trackable. `command_id` is
/// chosen by the client: the server tags the resulting events with it (so
/// the client knows its prediction was confirmed) and ignores repeats (so
/// resending after a dropped connection is safe).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub command_id: Uuid,
    /// Free-text name of whoever issued it, for the log ("DM", "Travis").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub command: CombatCommand,
}

impl CommandEnvelope {
    pub fn new(command: CombatCommand) -> Self {
        Self {
            command_id: Uuid::new_v4(),
            actor: None,
            command,
        }
    }

    pub fn by(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }
}

/// One entry in a combat's permanent log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedEvent {
    /// Position in the log, starting at 1 with no gaps.
    pub seq: u64,
    /// The command that caused it; `None` for the `Created` event.
    pub command_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub at: DateTime<Utc>,
    pub event: CombatEvent,
}

/// The current state of a combat as of log position `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatSnapshot {
    pub id: Uuid,
    pub seq: u64,
    pub state: CombatState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatSummary {
    pub id: Uuid,
    pub campaign: String,
    pub name: String,
    pub status: CombatStatus,
    pub round: u32,
    pub created_at: DateTime<Utc>,
    /// When the last event happened.
    pub updated_at: DateTime<Utc>,
}

/// Sent by the server over a combat's WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Sent on connect, and again if this client fell too far behind.
    Snapshot(CombatSnapshot),
    /// Broadcast to everyone, in log order.
    Event(LoggedEvent),
    /// Sent only to the client whose command was refused.
    Rejected { command_id: Uuid, error: String },
    /// The command was already applied earlier (e.g. resent after a
    /// reconnect); its events are already part of the state.
    AlreadyApplied { command_id: Uuid },
}

/// Sent by a client over a combat's WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ClientMessage {
    Command(CommandEnvelope),
}
