//! Shared domain types for Edda.
//!
//! This crate is deliberately free of server-side dependencies so it can be
//! used by every service *and* every frontend (native, CLI, or WASM).

pub mod action;
pub mod admin;
/// Character sheets: classes, abilities, skills, hit points and items.
pub mod character;
pub mod combat;
pub mod edition;
pub mod encounter;
pub mod monster;
/// Limited uses (spell slots, class features) and rests.
pub mod resources;
pub mod rules_text;
mod slots;
/// Spells known, spell slots and Pact Magic, and how many slots classes give.
pub mod spellcasting;

pub use action::{ActionError, CharacterAction};
pub use character::{
    Ability, AbilityScores, Character, CharacterSheet, CharacterSummary, ClassLevel, HitPoints,
    Item, Skill, ValidationError,
};
pub use edition::Edition;
pub use resources::{Pool, Recharge, Resource, RestKind};
pub use spellcasting::{PactMagic, Spell, Spellcasting};
