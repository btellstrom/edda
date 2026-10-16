//! Shared domain types for the campaign manager.
//!
//! This crate is deliberately free of server-side dependencies so it can be
//! used by every service *and* every frontend (native, CLI, or WASM).

pub mod action;
pub mod character;
pub mod resources;
pub mod spellcasting;

pub use action::{ActionError, CharacterAction};
pub use character::{
    Ability, AbilityScores, Character, CharacterSheet, CharacterSummary, ClassLevel, HitPoints,
    Item, Skill, ValidationError,
};
pub use resources::{Pool, Recharge, Resource, RestKind};
pub use spellcasting::{PactMagic, Spell, Spellcasting};
