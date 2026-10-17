//! In-play state changes to a character sheet: casting spells, spending
//! resources and resting. Defined here so every client and service shares
//! exactly the same rules.

use serde::{Deserialize, Serialize};

use crate::{CharacterSheet, Pool, resources::RestKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CharacterAction {
    /// Cast a known spell, expending the appropriate slot. Cantrips and
    /// ritual casts don't use a slot.
    CastSpell {
        spell: String,
        /// Slot level to cast at (upcasting). Defaults to the spell's level.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_level: Option<u8>,
        /// Use a Pact Magic slot instead of a regular one.
        #[serde(default)]
        pact: bool,
        /// Cast as a ritual (spell must have the ritual tag).
        #[serde(default)]
        ritual: bool,
    },
    ExpendSpellSlot {
        level: u8,
    },
    RestoreSpellSlot {
        level: u8,
    },
    ExpendPactSlot,
    RestorePactSlot,
    UseResource {
        name: String,
        #[serde(default = "one")]
        amount: u16,
    },
    RestoreResource {
        name: String,
        #[serde(default = "one")]
        amount: u16,
    },
    Rest {
        kind: RestKind,
    },
}

fn one() -> u16 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ActionError {
    #[error("unknown spell: {0}")]
    UnknownSpell(String),
    #[error("{0} is not prepared")]
    SpellNotPrepared(String),
    #[error("{0} cannot be cast as a ritual")]
    NotARitual(String),
    #[error("{spell} is level {spell_level} and cannot be cast with a level {slot_level} slot")]
    SlotTooLow {
        spell: String,
        spell_level: u8,
        slot_level: u8,
    },
    #[error("no level {0} spell slots")]
    NoSlotsOfLevel(u8),
    #[error("no level {0} spell slots remaining")]
    NoSlotsRemaining(u8),
    #[error("character has no pact magic")]
    NoPactMagic,
    #[error("pact slots are level {pact_level}, not {requested}")]
    PactLevelMismatch { pact_level: u8, requested: u8 },
    #[error("no pact magic slots remaining")]
    NoPactSlotsRemaining,
    #[error("unknown resource: {0}")]
    UnknownResource(String),
    #[error("{name} has {remaining} uses remaining, {requested} requested")]
    NotEnoughUses {
        name: String,
        remaining: u16,
        requested: u16,
    },
}

impl ActionError {
    /// True when the action was well-formed but the character has run out
    /// of whatever it needed. Lets clients distinguish "you're out of slots"
    /// from "that spell doesn't exist".
    pub fn is_exhausted(&self) -> bool {
        matches!(
            self,
            ActionError::NoSlotsRemaining(_)
                | ActionError::NoPactSlotsRemaining
                | ActionError::NotEnoughUses { .. }
        )
    }
}

impl CharacterSheet {
    /// Applies an action. On error the sheet is left unchanged.
    pub fn apply(&mut self, action: &CharacterAction) -> Result<(), ActionError> {
        match action {
            CharacterAction::CastSpell {
                spell,
                at_level,
                pact,
                ritual,
            } => self.cast_spell(spell, *at_level, *pact, *ritual),
            CharacterAction::ExpendSpellSlot { level } => self.expend_slot(*level),
            CharacterAction::RestoreSpellSlot { level } => {
                self.slot_mut(*level)?.restore(1);
                Ok(())
            }
            CharacterAction::ExpendPactSlot => self.expend_pact_slot(),
            CharacterAction::RestorePactSlot => {
                let pact = self.spellcasting.pact_magic.as_mut();
                pact.ok_or(ActionError::NoPactMagic)?.slots.restore(1);
                Ok(())
            }
            CharacterAction::UseResource { name, amount } => {
                let resource = self.resource_mut(name)?;
                let remaining = resource.uses.remaining();
                if !resource.uses.expend(*amount) {
                    return Err(ActionError::NotEnoughUses {
                        name: resource.name.clone(),
                        remaining,
                        requested: *amount,
                    });
                }
                Ok(())
            }
            CharacterAction::RestoreResource { name, amount } => {
                self.resource_mut(name)?.uses.restore(*amount);
                Ok(())
            }
            CharacterAction::Rest { kind } => {
                self.rest(*kind);
                Ok(())
            }
        }
    }

    pub fn rest(&mut self, kind: RestKind) {
        for resource in &mut self.resources {
            if resource.recharge.recovers_on(kind) {
                resource.uses.reset();
            }
        }
        if let Some(pact) = &mut self.spellcasting.pact_magic {
            pact.slots.reset();
        }
        if kind == RestKind::Long {
            self.spellcasting.slots.values_mut().for_each(Pool::reset);
            self.hit_points.current = self.hit_points.max;
        }
    }

    fn cast_spell(
        &mut self,
        name: &str,
        at_level: Option<u8>,
        pact: bool,
        ritual: bool,
    ) -> Result<(), ActionError> {
        let spell = self
            .spellcasting
            .find_spell(name)
            .ok_or_else(|| ActionError::UnknownSpell(name.to_string()))?;
        let (spell_name, spell_level) = (spell.name.clone(), spell.level);

        if ritual {
            // Rituals need neither a slot nor (for Wizards) preparation, so
            // check the tag before anything else.
            return if spell.ritual {
                Ok(())
            } else {
                Err(ActionError::NotARitual(spell_name))
            };
        }
        if !spell.prepared {
            return Err(ActionError::SpellNotPrepared(spell_name));
        }
        if spell_level == 0 {
            return Ok(());
        }

        if pact {
            let pact_magic = self
                .spellcasting
                .pact_magic
                .ok_or(ActionError::NoPactMagic)?;
            let slot_level = pact_magic.slot_level;
            if let Some(requested) = at_level.filter(|&l| l != slot_level) {
                return Err(ActionError::PactLevelMismatch {
                    pact_level: slot_level,
                    requested,
                });
            }
            if slot_level < spell_level {
                return Err(ActionError::SlotTooLow {
                    spell: spell_name,
                    spell_level,
                    slot_level,
                });
            }
            return self.expend_pact_slot();
        }

        let slot_level = at_level.unwrap_or(spell_level);
        if slot_level < spell_level {
            return Err(ActionError::SlotTooLow {
                spell: spell_name,
                spell_level,
                slot_level,
            });
        }
        self.expend_slot(slot_level)
    }

    fn slot_mut(&mut self, level: u8) -> Result<&mut crate::Pool, ActionError> {
        self.spellcasting
            .slots
            .get_mut(&level)
            .filter(|s| s.max > 0)
            .ok_or(ActionError::NoSlotsOfLevel(level))
    }

    fn expend_slot(&mut self, level: u8) -> Result<(), ActionError> {
        if self.slot_mut(level)?.expend(1) {
            Ok(())
        } else {
            Err(ActionError::NoSlotsRemaining(level))
        }
    }

    fn expend_pact_slot(&mut self) -> Result<(), ActionError> {
        let pact = self.spellcasting.pact_magic.as_mut();
        if pact.ok_or(ActionError::NoPactMagic)?.slots.expend(1) {
            Ok(())
        } else {
            Err(ActionError::NoPactSlotsRemaining)
        }
    }

    fn resource_mut(&mut self, name: &str) -> Result<&mut crate::Resource, ActionError> {
        self.resources
            .iter_mut()
            .find(|r| r.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| ActionError::UnknownResource(name.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pool, Recharge, Resource, Spell, spellcasting::PactMagic};

    fn spell(name: &str, level: u8) -> Spell {
        Spell {
            name: name.into(),
            level,
            class: None,
            prepared: true,
            concentration: false,
            ritual: false,
            notes: String::new(),
        }
    }

    fn eldritch_knight_warlock() -> CharacterSheet {
        let mut s = CharacterSheet {
            name: "Test".into(),
            ..Default::default()
        };
        s.spellcasting.spells = vec![
            spell("Fire Bolt", 0),
            spell("Shield", 1),
            spell("Hex", 1),
            Spell {
                ritual: true,
                prepared: false,
                ..spell("Detect Magic", 1)
            },
        ];
        s.spellcasting.slots.insert(1, Pool::new(2));
        s.spellcasting.slots.insert(2, Pool::new(1));
        s.spellcasting.pact_magic = Some(PactMagic {
            slot_level: 2,
            slots: Pool::new(1),
        });
        s.resources.push(Resource {
            name: "Action Surge".into(),
            uses: Pool::new(1),
            recharge: Recharge::ShortRest,
            notes: String::new(),
        });
        s.resources.push(Resource {
            name: "Indomitable".into(),
            uses: Pool::new(1),
            recharge: Recharge::LongRest,
            notes: String::new(),
        });
        s
    }

    fn cast(spell: &str) -> CharacterAction {
        CharacterAction::CastSpell {
            spell: spell.into(),
            at_level: None,
            pact: false,
            ritual: false,
        }
    }

    #[test]
    fn casting_spends_slots_until_exhausted() {
        let mut s = eldritch_knight_warlock();
        s.apply(&cast("shield")).unwrap();
        s.apply(&cast("Shield")).unwrap();
        let err = s.apply(&cast("Shield")).unwrap_err();
        assert_eq!(err, ActionError::NoSlotsRemaining(1));
        assert!(err.is_exhausted());
    }

    #[test]
    fn cantrips_and_rituals_are_free() {
        let mut s = eldritch_knight_warlock();
        let before = s.clone();
        s.apply(&cast("Fire Bolt")).unwrap();
        s.apply(&CharacterAction::CastSpell {
            spell: "Detect Magic".into(),
            at_level: None,
            pact: false,
            ritual: true,
        })
        .unwrap();
        assert_eq!(s, before);
        // Not prepared, so it can't be cast normally.
        assert!(matches!(
            s.apply(&cast("Detect Magic")),
            Err(ActionError::SpellNotPrepared(_))
        ));
    }

    #[test]
    fn upcasting_and_pact_slots() {
        let mut s = eldritch_knight_warlock();
        let upcast = CharacterAction::CastSpell {
            spell: "Hex".into(),
            at_level: Some(2),
            pact: false,
            ritual: false,
        };
        s.apply(&upcast).unwrap();
        assert_eq!(s.spellcasting.slots[&2].used, 1);
        assert_eq!(s.spellcasting.slots[&1].used, 0);

        let pact = CharacterAction::CastSpell {
            spell: "Hex".into(),
            at_level: None,
            pact: true,
            ritual: false,
        };
        s.apply(&pact).unwrap();
        assert_eq!(s.apply(&pact), Err(ActionError::NoPactSlotsRemaining));
    }

    #[test]
    fn short_rest_restores_short_rest_resources_and_pact_slots_only() {
        let mut s = eldritch_knight_warlock();
        for action in [
            cast("Shield"),
            CharacterAction::ExpendPactSlot,
            CharacterAction::UseResource {
                name: "action surge".into(),
                amount: 1,
            },
            CharacterAction::UseResource {
                name: "Indomitable".into(),
                amount: 1,
            },
        ] {
            s.apply(&action).unwrap();
        }
        assert!(matches!(
            s.apply(&CharacterAction::UseResource {
                name: "Action Surge".into(),
                amount: 1
            }),
            Err(ActionError::NotEnoughUses { remaining: 0, .. })
        ));

        s.rest(RestKind::Short);
        assert_eq!(s.resources[0].uses.used, 0, "action surge back");
        assert_eq!(s.resources[1].uses.used, 1, "indomitable still spent");
        assert_eq!(s.spellcasting.pact_magic.unwrap().slots.used, 0);
        assert_eq!(s.spellcasting.slots[&1].used, 1, "regular slot still spent");

        s.rest(RestKind::Long);
        assert_eq!(s.resources[1].uses.used, 0);
        assert_eq!(s.spellcasting.slots[&1].used, 0);
    }

    #[test]
    fn failed_actions_leave_sheet_unchanged() {
        let mut s = eldritch_knight_warlock();
        let before = s.clone();
        assert!(s.apply(&cast("Wish")).is_err());
        assert!(
            s.apply(&CharacterAction::UseResource {
                name: "Action Surge".into(),
                amount: 2
            })
            .is_err()
        );
        assert!(
            s.apply(&CharacterAction::ExpendSpellSlot { level: 3 })
                .is_err()
        );
        assert_eq!(s, before);
    }

    #[test]
    fn action_json_shape() {
        let action: CharacterAction =
            serde_json::from_str(r#"{"type":"use_resource","name":"Action Surge"}"#).unwrap();
        assert_eq!(
            action,
            CharacterAction::UseResource {
                name: "Action Surge".into(),
                amount: 1
            }
        );
        let rest: CharacterAction =
            serde_json::from_str(r#"{"type":"rest","kind":"short"}"#).unwrap();
        assert_eq!(
            rest,
            CharacterAction::Rest {
                kind: RestKind::Short
            }
        );
    }
}
