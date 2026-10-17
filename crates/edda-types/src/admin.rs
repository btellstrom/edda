//! What the services' admin APIs answer (see `edda_server::admin`): what
//! they deleted and took in, and each one's part of a campaign as it's
//! exported, to keep or to move to another server (the auth service's is
//! `edda_auth::protocol::AuthExport`).

use serde::{Deserialize, Serialize};

/// `DELETE /admin/campaigns/{id}`: how many of the campaign's things a
/// service deleted (characters, combats, monsters and texts, or devices).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Deleted {
    pub deleted: u64,
}

/// `PUT /admin/campaigns/{id}/import`: how many of the campaign's things a
/// service took in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Imported {
    pub imported: u64,
}

/// The character service's part of a campaign, as exported and imported:
/// `GET`/`PUT /admin/campaigns/{id}/export|import`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CharactersExport {
    pub characters: Vec<crate::Character>,
}

/// The combat service's part of a campaign: its combats, log and all, and
/// its planned encounters.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CombatExport {
    pub combats: Vec<ExportedCombat>,
    pub encounters: Vec<crate::encounter::Encounter>,
}

/// A combat as exported: everything else about it follows from its log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExportedCombat {
    pub id: uuid::Uuid,
    pub log: Vec<crate::combat::LoggedEvent>,
    /// The commands it has run, and whose device sent them, so resent ones
    /// are still recognised and each device undoes only its own.
    pub commands: Vec<SentCommand>,
    /// How far its hit points have reached the character sheets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hp_synced: Option<u64>,
}

/// A command a combat has run, and the key of the device that sent it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SentCommand {
    pub id: uuid::Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
}

/// The monster service's part of a campaign: its own monsters, spells and
/// conditions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MonstersExport {
    pub monsters: Vec<crate::monster::Monster>,
    pub spells: Vec<crate::rules_text::Entry<crate::rules_text::SpellText>>,
    pub conditions: Vec<crate::rules_text::Entry<crate::rules_text::ConditionText>>,
}
