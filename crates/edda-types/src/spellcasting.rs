use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ValidationError, resources::Pool};

/// Everything about a character's spellcasting. Empty for non-casters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct Spellcasting {
    pub spells: Vec<Spell>,
    /// Regular spell slots keyed by slot level (1-9). Recover on a long rest.
    #[serde(deserialize_with = "level_keys::deserialize")]
    // JSON keys are strings: "1" to "9".
    #[cfg_attr(feature = "openapi", schema(value_type = BTreeMap<String, Pool>))]
    pub slots: BTreeMap<u8, Pool>,
    /// Warlock Pact Magic slots. Recover on a short or long rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pact_magic: Option<PactMagic>,
}

/// A spell the character knows or has prepared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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

/// Warlock Pact Magic: slots all of one level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PactMagic {
    /// All pact slots are this level (1-5).
    pub slot_level: u8,
    #[serde(flatten)]
    pub slots: Pool,
}

impl Spellcasting {
    /// The highest level of a spell slot.
    pub const MAX_SPELL_LEVEL: u8 = 9;
    /// The highest level of a pact slot.
    pub const MAX_PACT_LEVEL: u8 = 5;

    /// The spell called `name` (any case), if the character has it.
    pub fn find_spell(&self, name: &str) -> Option<&Spell> {
        self.spells
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Checks the spells' names and levels, and the slots' levels and
    /// counts.
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

/// JSON map keys are always strings, and inside `#[serde(flatten)]` (as in
/// `Character`) serde can't convert `"1"` back to an integer key on its own.
/// Accept slot levels as either integers or numeric strings.
mod level_keys {
    use std::{collections::BTreeMap, fmt};

    use serde::{Deserialize, Deserializer, de};

    use crate::resources::Pool;

    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    struct Level(u8);

    impl<'de> Deserialize<'de> for Level {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl de::Visitor<'_> for Visitor {
                type Value = Level;
                fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    f.write_str("a spell slot level")
                }
                fn visit_u64<E: de::Error>(self, v: u64) -> Result<Level, E> {
                    u8::try_from(v).map(Level).map_err(E::custom)
                }
                fn visit_str<E: de::Error>(self, v: &str) -> Result<Level, E> {
                    v.parse().map(Level).map_err(E::custom)
                }
            }
            d.deserialize_any(Visitor)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<u8, Pool>, D::Error> {
        let map = BTreeMap::<Level, Pool>::deserialize(d)?;
        Ok(map.into_iter().map(|(Level(l), p)| (l, p)).collect())
    }
}
