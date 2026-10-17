//! How many spell slots a character's classes give.
//!
//! Slots aren't entered by hand: they follow from the classes. A character
//! with one spellcasting class uses that class's own table. With several,
//! each class adds to a combined caster level (full casters their level,
//! half and third casters a share of it) and the multiclass table gives one
//! shared pool, usable for any of the classes' spells. Warlock Pact Magic
//! is separate either way: its slots come from Warlock levels alone.

use std::collections::BTreeMap;

use crate::{CharacterSheet, ClassLevel, Edition, PactMagic, Pool};

/// How a class gains spell slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progression {
    /// Bard, Cleric, Druid, Sorcerer, Wizard.
    Full,
    /// Paladin, Ranger.
    Half,
    /// Artificer: half, but from 1st level and rounded up.
    Artificer,
    /// Eldritch Knight Fighters and Arcane Trickster Rogues.
    Third,
    /// Warlock: Pact Magic instead of spell slots.
    Pact,
}

/// Classes are recognised by name (and for third casters, subclass);
/// others, homebrew included, give no slots.
fn progression(class: &ClassLevel) -> Option<Progression> {
    let name = class.name.trim().to_lowercase();
    let subclass = class.subclass.as_deref().unwrap_or("").to_lowercase();
    match name.as_str() {
        "bard" | "cleric" | "druid" | "sorcerer" | "wizard" => Some(Progression::Full),
        "paladin" | "ranger" => Some(Progression::Half),
        "artificer" => Some(Progression::Artificer),
        "warlock" => Some(Progression::Pact),
        "fighter" if subclass.contains("eldritch knight") => Some(Progression::Third),
        "rogue" if subclass.contains("arcane trickster") => Some(Progression::Third),
        _ => None,
    }
}

/// Slots per slot level (1st to 9th) by caster level (1 to 20); the same
/// in both editions.
const SLOTS: [[u16; 9]; 20] = [
    [2, 0, 0, 0, 0, 0, 0, 0, 0],
    [3, 0, 0, 0, 0, 0, 0, 0, 0],
    [4, 2, 0, 0, 0, 0, 0, 0, 0],
    [4, 3, 0, 0, 0, 0, 0, 0, 0],
    [4, 3, 2, 0, 0, 0, 0, 0, 0],
    [4, 3, 3, 0, 0, 0, 0, 0, 0],
    [4, 3, 3, 1, 0, 0, 0, 0, 0],
    [4, 3, 3, 2, 0, 0, 0, 0, 0],
    [4, 3, 3, 3, 1, 0, 0, 0, 0],
    [4, 3, 3, 3, 2, 0, 0, 0, 0],
    [4, 3, 3, 3, 2, 1, 0, 0, 0],
    [4, 3, 3, 3, 2, 1, 0, 0, 0],
    [4, 3, 3, 3, 2, 1, 1, 0, 0],
    [4, 3, 3, 3, 2, 1, 1, 0, 0],
    [4, 3, 3, 3, 2, 1, 1, 1, 0],
    [4, 3, 3, 3, 2, 1, 1, 1, 0],
    [4, 3, 3, 3, 2, 1, 1, 1, 1],
    [4, 3, 3, 3, 3, 1, 1, 1, 1],
    [4, 3, 3, 3, 3, 2, 1, 1, 1],
    [4, 3, 3, 3, 3, 2, 2, 1, 1],
];

/// The caster level whose row of [`SLOTS`] the classes get.
fn caster_level(classes: &[ClassLevel], edition: Edition) -> u8 {
    let casters: Vec<(Progression, u8)> = classes
        .iter()
        .filter_map(|c| progression(c).map(|p| (p, c.level)))
        .filter(|(p, _)| *p != Progression::Pact)
        .collect();
    let level = match casters.as_slice() {
        // One class: its own table. Half and third casters' tables match the
        // shared one at their level divided and rounded up, from the level
        // they start casting at.
        [(progression, level)] => {
            let level = *level;
            match progression {
                Progression::Full => level,
                Progression::Half if edition == Edition::Rules2014 && level < 2 => 0,
                Progression::Half | Progression::Artificer => level.div_ceil(2),
                Progression::Third if level < 3 => 0,
                Progression::Third => level.div_ceil(3),
                Progression::Pact => 0,
            }
        }
        // Several: a combined caster level. Paladins and Rangers count half
        // their levels, rounded down in 2014 and up in 2024.
        many => many
            .iter()
            .map(|&(progression, level)| match progression {
                Progression::Full => level,
                Progression::Half if edition == Edition::Rules2014 => level / 2,
                Progression::Half | Progression::Artificer => level.div_ceil(2),
                Progression::Third => level / 3,
                Progression::Pact => 0,
            })
            .sum(),
    };
    level.min(20)
}

/// Warlock levels give Pact Magic: (slot level, number of slots).
fn pact_slots(classes: &[ClassLevel]) -> Option<(u8, u16)> {
    let level: u8 = classes
        .iter()
        .filter(|c| progression(c) == Some(Progression::Pact))
        .map(|c| c.level)
        .sum();
    let count = match level {
        0 => return None,
        1 => 1,
        2..=10 => 2,
        11..=16 => 3,
        _ => 4,
    };
    Some((level.div_ceil(2).min(5), count))
}

impl CharacterSheet {
    /// Sets the spell slots (and Pact Magic) the classes give, under the
    /// campaign's `edition`. Slots already spent stay spent, up to the new
    /// number.
    pub fn derive_slots(&mut self, edition: Edition) {
        let row = match caster_level(&self.classes, edition) {
            0 => [0; 9],
            level => SLOTS[usize::from(level) - 1],
        };
        let casting = &mut self.spellcasting;
        let old = std::mem::take(&mut casting.slots);
        casting.slots = (1u8..)
            .zip(row)
            .filter(|&(_, max)| max > 0)
            .map(|(level, max)| {
                let used = old.get(&level).map_or(0, |p| p.used.min(max));
                (level, Pool { max, used })
            })
            .collect::<BTreeMap<_, _>>();
        casting.pact_magic = pact_slots(&self.classes).map(|(slot_level, max)| {
            let used = casting.pact_magic.map_or(0, |p| p.slots.used.min(max));
            PactMagic {
                slot_level,
                slots: Pool { max, used },
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str, level: u8) -> ClassLevel {
        ClassLevel {
            name: name.into(),
            subclass: None,
            level,
            spellcasting_ability: None,
        }
    }

    fn sub(name: &str, subclass: &str, level: u8) -> ClassLevel {
        ClassLevel {
            subclass: Some(subclass.into()),
            ..class(name, level)
        }
    }

    /// Slot counts from 1st level up, as derived.
    fn slots(classes: &[ClassLevel], edition: Edition) -> Vec<u16> {
        let mut sheet = CharacterSheet {
            classes: classes.to_vec(),
            ..Default::default()
        };
        sheet.derive_slots(edition);
        sheet.spellcasting.slots.values().map(|p| p.max).collect()
    }

    use Edition::{Rules2014 as E14, Rules2024 as E24};

    #[test]
    fn single_class_casters_use_their_own_table() {
        assert_eq!(slots(&[class("Wizard", 1)], E24), [2]);
        assert_eq!(slots(&[class("Cleric", 5)], E24), [4, 3, 2]);
        assert_eq!(
            slots(&[class("Druid", 20)], E14),
            [4, 3, 3, 3, 3, 2, 2, 1, 1]
        );
        assert_eq!(slots(&[class("Ranger", 5)], E24), [4, 2]);
        assert_eq!(slots(&[class("Paladin", 3)], E14), [3]);
        assert_eq!(slots(&[class("Paladin", 20)], E24), [4, 3, 3, 3, 2]);
        assert_eq!(slots(&[sub("Fighter", "Eldritch Knight", 7)], E24), [4, 2]);
        assert_eq!(slots(&[sub("Rogue", "Arcane Trickster", 3)], E14), [2]);
        assert!(slots(&[sub("Fighter", "Champion", 7)], E24).is_empty());
        assert!(slots(&[class("Barbarian", 10)], E24).is_empty());
        assert!(
            slots(&[class("Blood Hunter", 10)], E24).is_empty(),
            "homebrew"
        );
    }

    #[test]
    fn the_editions_differ_on_when_half_casters_start() {
        assert!(slots(&[class("Ranger", 1)], E14).is_empty());
        assert_eq!(slots(&[class("Ranger", 1)], E24), [2]);
        assert_eq!(slots(&[class("Artificer", 1)], E14), [2], "always from 1st");
        assert!(slots(&[sub("Fighter", "Eldritch Knight", 2)], E24).is_empty());
    }

    #[test]
    fn multiclass_casters_share_one_pool_from_their_combined_level() {
        // Wizard 3 + Cleric 2: a 5th-level caster.
        assert_eq!(
            slots(&[class("Wizard", 3), class("Cleric", 2)], E24),
            [4, 3, 2]
        );
        // Paladin 5 counts 2 in 2014 (round down), 3 in 2024 (round up).
        let paladin_sorcerer = [class("Paladin", 5), class("Sorcerer", 3)];
        assert_eq!(slots(&paladin_sorcerer, E14), [4, 3, 2]);
        assert_eq!(slots(&paladin_sorcerer, E24), [4, 3, 3]);
        // Third casters round down when multiclassing.
        let knight_wizard = [sub("Fighter", "Eldritch Knight", 5), class("Wizard", 1)];
        assert_eq!(slots(&knight_wizard, E24), [3]);
        // A level that adds nothing still makes it a multiclass: Paladin 1
        // counts 0 in 2014, leaving the Wizard's 1.
        assert_eq!(slots(&[class("Paladin", 1), class("Wizard", 1)], E14), [2]);
        // Classes without spells don't count.
        assert_eq!(
            slots(&[class("Wizard", 3), class("Fighter", 5)], E24),
            [4, 2]
        );
    }

    #[test]
    fn warlock_pact_magic_stays_separate() {
        let mut sheet = CharacterSheet {
            classes: vec![class("Warlock", 5), class("Sorcerer", 3)],
            ..Default::default()
        };
        sheet.derive_slots(E24);
        let pact = sheet.spellcasting.pact_magic.unwrap();
        assert_eq!((pact.slot_level, pact.slots.max), (3, 2));
        let shared: Vec<_> = sheet.spellcasting.slots.values().map(|p| p.max).collect();
        assert_eq!(shared, [4, 2], "the Sorcerer's own table");

        sheet.classes = vec![class("Warlock", 11)];
        sheet.derive_slots(E24);
        let pact = sheet.spellcasting.pact_magic.unwrap();
        assert_eq!((pact.slot_level, pact.slots.max), (5, 3));
        assert!(sheet.spellcasting.slots.is_empty());

        sheet.classes = vec![class("Fighter", 1)];
        sheet.derive_slots(E24);
        assert_eq!(sheet.spellcasting.pact_magic, None);
    }

    #[test]
    fn spent_slots_stay_spent_up_to_the_new_number() {
        let mut sheet = CharacterSheet {
            classes: vec![class("Wizard", 5)],
            ..Default::default()
        };
        sheet.derive_slots(E24);
        sheet.spellcasting.slots.get_mut(&1).unwrap().used = 4;
        sheet.spellcasting.slots.get_mut(&3).unwrap().used = 1;
        // Hand-entered slots the classes don't give are dropped.
        sheet.spellcasting.slots.insert(9, Pool::new(5));

        sheet.classes[0].level = 1;
        sheet.derive_slots(E24);
        assert_eq!(
            sheet.spellcasting.slots,
            BTreeMap::from([(1, Pool { max: 2, used: 2 })])
        );
        sheet.classes[0].level = 5;
        sheet.derive_slots(E24);
        assert_eq!(sheet.spellcasting.slots[&1], Pool { max: 4, used: 2 });
        assert_eq!(sheet.spellcasting.slots[&3], Pool { max: 2, used: 0 });
    }
}
