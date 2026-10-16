//! Combat encounters, modelled with event sourcing.
//!
//! Clients send [`CombatCommand`]s (intents). [`decide`] turns a command into
//! [`CombatEvent`]s (facts), and [`CombatState::apply`] folds events into the
//! current state. The server is authoritative: it alone decides events and
//! fixes their order in the log. Because both functions live here, clients
//! run them too, showing the outcome of their own commands immediately and
//! reconciling when the server's events arrive.

mod protocol;
mod rules;

pub use protocol::{
    ClientMessage, CombatSnapshot, CombatSummary, CommandEnvelope, LoggedEvent, ServerMessage,
};
pub use rules::{CombatCommand, CombatError, Dice, NewEffect, NoDice, decide};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::HitPoints;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CombatState {
    pub name: String,
    /// The campaign this combat belongs to.
    #[serde(default)]
    pub campaign: String,
    pub status: CombatStatus,
    /// 0 until the combat starts.
    pub round: u32,
    /// Whose turn it is.
    pub current_turn: Option<Uuid>,
    /// Always kept in initiative order; combatants without initiative last.
    pub combatants: Vec<Combatant>,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombatStatus {
    /// Adding combatants and rolling initiative.
    #[default]
    Preparing,
    Active,
    /// Interrupted, e.g. at the end of a session; resumes exactly where it
    /// left off, however much later.
    Paused,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Combatant {
    /// Chosen by the client that adds the combatant, so it can predict the
    /// outcome exactly. The server rejects duplicates.
    pub id: Uuid,
    pub name: String,
    /// Set when this combatant is a character from the character service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_id: Option<Uuid>,
    #[serde(default)]
    pub initiative_modifier: i8,
    /// Initiative total. `None` until rolled or set.
    #[serde(default)]
    pub initiative: Option<i16>,
    pub hit_points: HitPoints,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armor_class: Option<u8>,
}

impl Combatant {
    pub fn new(name: impl Into<String>, max_hp: u16) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            character_id: None,
            initiative_modifier: 0,
            initiative: None,
            hit_points: HitPoints {
                max: max_hp,
                current: max_hp,
                temporary: 0,
            },
            armor_class: None,
        }
    }

    /// A combatant for a character sheet, using its current HP, AC and
    /// Dexterity modifier.
    pub fn from_character(character: &crate::Character) -> Self {
        let sheet = &character.sheet;
        Self {
            id: Uuid::new_v4(),
            name: sheet.name.clone(),
            character_id: Some(character.id),
            initiative_modifier: sheet.ability_scores.modifier(crate::Ability::Dexterity),
            initiative: None,
            hit_points: sheet.hit_points,
            armor_class: Some(sheet.armor_class),
        }
    }

    pub fn is_down(&self) -> bool {
        self.hit_points.current == 0
    }
}

/// Anything in play with a duration or a target: spells, conditions, auras.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    pub id: Uuid,
    pub name: String,
    /// Who created it (e.g. the caster).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Uuid>,
    #[serde(default)]
    pub targets: Vec<Uuid>,
    /// A creature can concentrate on one effect at a time, and loses
    /// concentration when dropped to 0 HP.
    #[serde(default)]
    pub concentration: bool,
    /// `None` lasts until removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<Expiry>,
}

/// When an effect ends: at the start of `at_turn_of`'s turn in `round`, or
/// at the start of `round` if there's no such combatant (5e durations run
/// from the caster's turn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expiry {
    pub round: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_turn_of: Option<Uuid>,
}

/// Something that happened in a combat. Events are facts: they carry
/// outcomes (the rolled number, the resulting HP) so replaying the log never
/// needs to re-roll or re-decide anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CombatEvent {
    Created {
        name: String,
        #[serde(default)]
        campaign: String,
    },
    CombatantAdded {
        combatant: Combatant,
    },
    CombatantRemoved {
        combatant: Uuid,
    },
    InitiativeRolled {
        combatant: Uuid,
        roll: u8,
        modifier: i8,
        total: i16,
    },
    InitiativeSet {
        combatant: Uuid,
        total: i16,
    },
    Started,
    RoundStarted {
        round: u32,
    },
    TurnStarted {
        combatant: Uuid,
    },
    Damaged {
        combatant: Uuid,
        amount: u32,
        hit_points: HitPoints,
    },
    Healed {
        combatant: Uuid,
        amount: u32,
        hit_points: HitPoints,
    },
    TemporaryHitPointsSet {
        combatant: Uuid,
        hit_points: HitPoints,
    },
    EffectAdded {
        effect: Effect,
    },
    EffectEnded {
        effect: Uuid,
        reason: EffectEndReason,
    },
    Paused,
    Resumed,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectEndReason {
    Removed,
    Expired,
    /// The source started concentrating on something else.
    ConcentrationReplaced,
    /// The source dropped to 0 HP.
    ConcentrationBroken,
    /// The source left the combat.
    SourceRemoved,
}

impl CombatState {
    /// Rebuilds a combat from its full event log.
    pub fn replay<'a>(events: impl IntoIterator<Item = &'a CombatEvent>) -> Self {
        let mut state = Self::default();
        for event in events {
            state.apply(event);
        }
        state
    }

    pub fn combatant(&self, id: Uuid) -> Option<&Combatant> {
        self.combatants.iter().find(|c| c.id == id)
    }

    pub fn effect(&self, id: Uuid) -> Option<&Effect> {
        self.effects.iter().find(|e| e.id == id)
    }

    /// Combatants who take turns, in turn order.
    pub fn turn_order(&self) -> impl Iterator<Item = &Combatant> {
        self.combatants.iter().filter(|c| c.initiative.is_some())
    }

    /// Applies an event. Events are facts already validated by [`decide`],
    /// so this never fails; references to unknown ids are ignored.
    pub fn apply(&mut self, event: &CombatEvent) {
        match event {
            CombatEvent::Created { name, campaign } => {
                self.name.clone_from(name);
                self.campaign.clone_from(campaign);
            }
            CombatEvent::CombatantAdded { combatant } => {
                self.combatants.push(combatant.clone());
                self.sort();
            }
            CombatEvent::CombatantRemoved { combatant } => {
                self.combatants.retain(|c| c.id != *combatant);
                for effect in &mut self.effects {
                    effect.targets.retain(|t| t != combatant);
                }
                if self.current_turn == Some(*combatant) {
                    self.current_turn = None;
                }
            }
            CombatEvent::InitiativeRolled {
                combatant, total, ..
            }
            | CombatEvent::InitiativeSet { combatant, total } => {
                if let Some(c) = self.combatant_mut(*combatant) {
                    c.initiative = Some(*total);
                }
                self.sort();
            }
            CombatEvent::Started => self.status = CombatStatus::Active,
            CombatEvent::RoundStarted { round } => self.round = *round,
            CombatEvent::TurnStarted { combatant } => self.current_turn = Some(*combatant),
            CombatEvent::Damaged {
                combatant,
                hit_points,
                ..
            }
            | CombatEvent::Healed {
                combatant,
                hit_points,
                ..
            }
            | CombatEvent::TemporaryHitPointsSet {
                combatant,
                hit_points,
            } => {
                if let Some(c) = self.combatant_mut(*combatant) {
                    c.hit_points = *hit_points;
                }
            }
            CombatEvent::EffectAdded { effect } => self.effects.push(effect.clone()),
            CombatEvent::EffectEnded { effect, .. } => self.effects.retain(|e| e.id != *effect),
            CombatEvent::Paused => self.status = CombatStatus::Paused,
            CombatEvent::Resumed => self.status = CombatStatus::Active,
            CombatEvent::Ended => {
                self.status = CombatStatus::Ended;
                self.current_turn = None;
            }
        }
    }

    fn combatant_mut(&mut self, id: Uuid) -> Option<&mut Combatant> {
        self.combatants.iter_mut().find(|c| c.id == id)
    }

    /// Highest initiative first, ties to the higher modifier, then to
    /// whoever joined first (the sort is stable). Deterministic, so clients
    /// and server always agree on the order.
    fn sort(&mut self) {
        self.combatants.sort_by(|a, b| {
            let key = |c: &Combatant| (c.initiative.is_some(), c.initiative, c.initiative_modifier);
            key(b).cmp(&key(a))
        });
    }
}
