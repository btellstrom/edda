use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ValidationError, resources::Pool};

/// Everything about a character's spellcasting. Empty for non-casters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spellcasting {
    pub spells: Vec<Spell>,
    /// Regular spell slots keyed by slot level (1-9). Recover on a long rest.
    pub slots: BTreeMap<u8, Pool>,
    /// Warlock Pact Magic slots. Recover on a short or long rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pact_magic: Option<PactMagic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spell {
    pub name: String,
    /// 0 for cantrips.
    #[serde(default)]
    pub level: u8,
    /// Which class grants this spell, for multiclass characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// Whether the spell can currently be cast. Defaults to true so "known"
    /// casters (Sorcerer, Bard, ...) needn't care; Wizards and Clerics toggle it.
    #[serde(default = "yes")]
    pub prepared: bool,
    #[serde(default)]
    pub concentration: bool,
    #[serde(default)]
    pub ritual: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PactMagic {
    /// All pact slots are this level (1-5).
    pub slot_level: u8,
    #[serde(flatten)]
    pub slots: Pool,
}

impl Spellcasting {
    pub const MAX_SPELL_LEVEL: u8 = 9;
    pub const MAX_PACT_LEVEL: u8 = 5;

    pub fn find_spell(&self, name: &str) -> Option<&Spell> {
        self.spells
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        for spell in &self.spells {
            if spell.name.trim().is_empty() {
                return Err(ValidationError::EmptySpellName);
            }
            if spell.level > Self::MAX_SPELL_LEVEL {
                return Err(ValidationError::InvalidSpellLevel(
                    spell.name.clone(),
                    spell.level,
                ));
            }
        }
        for (&level, pool) in &self.slots {
            if !(1..=Self::MAX_SPELL_LEVEL).contains(&level) {
                return Err(ValidationError::InvalidSlotLevel(level));
            }
            if !pool.is_valid() {
                return Err(ValidationError::Overspent(format!(
                    "level {level} spell slots"
                )));
            }
        }
        if let Some(pact) = &self.pact_magic {
            if !(1..=Self::MAX_PACT_LEVEL).contains(&pact.slot_level) {
                return Err(ValidationError::InvalidSlotLevel(pact.slot_level));
            }
            if !pact.slots.is_valid() {
                return Err(ValidationError::Overspent("pact magic slots".into()));
            }
        }
        Ok(())
    }
}
