//! What a combat command costs a character's sheet: a spell slot, or uses
//! of a feature. The combat service spends it on the sheet before running
//! the command, and gives it back when the command is refused or undone,
//! so every frontend gets that for free.

use uuid::Uuid;

use super::{CombatCommand, CombatEvent, SpellSlot};
use crate::CharacterAction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SheetCost {
    Slot(SpellSlot),
    Feature { name: String, amount: u16 },
}

impl SheetCost {
    /// The action that spends it.
    pub fn spend(&self) -> CharacterAction {
        match self {
            SheetCost::Slot(SpellSlot::Level(level)) => {
                CharacterAction::ExpendSpellSlot { level: *level }
            }
            SheetCost::Slot(SpellSlot::Pact(_)) => CharacterAction::ExpendPactSlot,
            SheetCost::Feature { name, amount } => CharacterAction::UseResource {
                name: name.clone(),
                amount: *amount,
            },
        }
    }

    /// The action that gives it back.
    pub fn give_back(&self) -> CharacterAction {
        match self {
            SheetCost::Slot(SpellSlot::Level(level)) => {
                CharacterAction::RestoreSpellSlot { level: *level }
            }
            SheetCost::Slot(SpellSlot::Pact(_)) => CharacterAction::RestorePactSlot,
            SheetCost::Feature { name, amount } => CharacterAction::RestoreResource {
                name: name.clone(),
                amount: *amount,
            },
        }
    }
}

impl CombatCommand {
    /// What running this costs the sheet of which combatant, if anything.
    pub fn sheet_cost(&self) -> Option<(Uuid, SheetCost)> {
        match self {
            CombatCommand::CastSpell {
                caster,
                slot: Some(slot),
                ..
            } => Some((*caster, SheetCost::Slot(*slot))),
            CombatCommand::UseFeature {
                combatant,
                feature,
                amount,
            } => Some((
                *combatant,
                SheetCost::Feature {
                    name: feature.clone(),
                    amount: *amount,
                },
            )),
            _ => None,
        }
    }
}

impl CombatEvent {
    /// What the command that logged this event cost a sheet: what undoing
    /// it gives back.
    pub fn sheet_cost(&self) -> Option<(Uuid, SheetCost)> {
        match self {
            CombatEvent::SpellCast {
                caster,
                slot: Some(slot),
                ..
            } => Some((*caster, SheetCost::Slot(*slot))),
            CombatEvent::FeatureUsed {
                combatant,
                feature,
                amount,
            } => Some((
                *combatant,
                SheetCost::Feature {
                    name: feature.clone(),
                    amount: *amount,
                },
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casts_and_feature_uses_cost_what_they_say_and_give_it_back() {
        let caster = Uuid::new_v4();
        let cast = CombatCommand::CastSpell {
            caster,
            spell: "Bless".into(),
            slot: Some(SpellSlot::Level(2)),
            targets: vec![],
            concentration: None,
        };
        let (who, cost) = cast.sheet_cost().unwrap();
        assert_eq!(who, caster);
        assert_eq!(cost.spend(), CharacterAction::ExpendSpellSlot { level: 2 });
        assert_eq!(
            cost.give_back(),
            CharacterAction::RestoreSpellSlot { level: 2 }
        );

        let pact = SheetCost::Slot(SpellSlot::Pact(3));
        assert_eq!(pact.spend(), CharacterAction::ExpendPactSlot);
        assert_eq!(pact.give_back(), CharacterAction::RestorePactSlot);

        let cantrip = CombatCommand::CastSpell {
            caster,
            spell: "Fire Bolt".into(),
            slot: None,
            targets: vec![],
            concentration: None,
        };
        assert_eq!(cantrip.sheet_cost(), None, "cantrips are free");

        let used = CombatEvent::FeatureUsed {
            combatant: caster,
            feature: "Rage".into(),
            amount: 1,
        };
        assert_eq!(
            used.sheet_cost().unwrap().1.give_back(),
            CharacterAction::RestoreResource {
                name: "Rage".into(),
                amount: 1
            }
        );
    }
}
