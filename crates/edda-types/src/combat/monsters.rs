//! Adding monsters: what [`super::CombatCommand::AddMonsters`] does, once
//! the server has the monster's stat block. They're named and numbered
//! here, and their hit points rolled with the server's dice, so every
//! frontend adds them the same way and two DMs adding goblins at once
//! can't both make a "Goblin 3".

use super::{CombatCommand, CombatError, CombatEvent, CombatState, Combatant, Dice, decide};
use crate::monster::Monster;

/// The events that add `count` of `monster` to the combat, at their
/// maximum hit points or, with `roll`, rolled.
pub fn add_monsters(
    state: &CombatState,
    monster: &Monster,
    count: u16,
    roll: bool,
    dice: &mut dyn Dice,
) -> Result<Vec<CombatEvent>, CombatError> {
    if count == 0 {
        return Err(CombatError::NothingToAdd);
    }
    let mut state = state.clone();
    let mut events = Vec::new();
    for name in monster_names(&monster.sheet.name, count, &state) {
        let hit_points = match (roll, monster.sheet.hit_dice()) {
            (true, Some(hit_dice)) => {
                let mut unavailable = false;
                let rolled = hit_dice.roll(|sides| {
                    dice.roll(sides).unwrap_or_else(|| {
                        unavailable = true;
                        1
                    })
                });
                if unavailable {
                    return Err(CombatError::DiceUnavailable);
                }
                rolled
            }
            _ => monster.sheet.max_hit_points(),
        };
        let combatant = Combatant::from_monster(monster, name, hit_points);
        for event in decide(&state, &CombatCommand::AddCombatant { combatant }, dice)? {
            state.apply(&event);
            events.push(event);
        }
    }
    Ok(events)
}

/// Names for `count` more of a monster: just its name for the only one in
/// the fight, otherwise numbered on from any already there ("Goblin 2").
pub fn monster_names(base: &str, count: u16, state: &CombatState) -> Vec<String> {
    let mut plain = false;
    let mut highest = 0;
    for combatant in &state.combatants {
        if combatant.name == base {
            plain = true;
        } else if let Some(n) = combatant
            .name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix(' '))
            .and_then(|n| n.parse::<u16>().ok())
        {
            highest = highest.max(n);
        }
    }
    if count == 1 && !plain && highest == 0 {
        return vec![base.to_string()];
    }
    let first = highest.max(u16::from(plain)) + 1;
    (first..first + count)
        .map(|n| format!("{base} {n}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::{
        combat::{CombatStatus, NoDice},
        monster::MonsterSheet,
    };

    /// Dice that always roll `0`'s number.
    struct Always(u16);

    impl Dice for Always {
        fn roll(&mut self, _sides: u16) -> Option<u16> {
            Some(self.0)
        }
    }

    fn goblin() -> Monster {
        Monster {
            id: Uuid::new_v4(),
            campaign: None,
            sheet: MonsterSheet {
                name: "Goblin".into(),
                hit_points: 7,
                hit_dice: "2d6".into(),
                ..Default::default()
            },
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn added(events: &[CombatEvent]) -> Vec<(String, u16)> {
        events
            .iter()
            .filter_map(|e| match e {
                CombatEvent::CombatantAdded { combatant } => {
                    Some((combatant.name.clone(), combatant.hit_points.max))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn monsters_join_numbered_at_their_maximum_or_rolled() {
        let goblin = goblin();
        let mut state = CombatState::default();
        let events = add_monsters(&state, &goblin, 1, false, &mut NoDice).unwrap();
        assert_eq!(added(&events), [("Goblin".to_string(), 12)]);
        for e in &events {
            state.apply(e);
        }

        let events = add_monsters(&state, &goblin, 2, true, &mut Always(3)).unwrap();
        assert_eq!(
            added(&events),
            [("Goblin 2".to_string(), 6), ("Goblin 3".to_string(), 6)]
        );
        let combatant = |e: &CombatEvent| match e {
            CombatEvent::CombatantAdded { combatant } => combatant.monster_id,
            _ => None,
        };
        assert!(events.iter().all(|e| combatant(e) == Some(goblin.id)));
    }

    #[test]
    fn rolling_needs_dice_and_some_must_be_added() {
        let state = CombatState::default();
        assert_eq!(
            add_monsters(&state, &goblin(), 2, true, &mut NoDice),
            Err(CombatError::DiceUnavailable)
        );
        assert_eq!(
            add_monsters(&state, &goblin(), 0, false, &mut NoDice),
            Err(CombatError::NothingToAdd)
        );
        let ended = CombatState {
            status: CombatStatus::Ended,
            ..Default::default()
        };
        assert_eq!(
            add_monsters(&ended, &goblin(), 1, false, &mut NoDice),
            Err(CombatError::Ended)
        );
    }

    #[test]
    fn monster_names_number_on_from_those_in_the_fight() {
        let mut state = CombatState::default();
        assert_eq!(monster_names("Rat", 1, &state), ["Rat"]);
        assert_eq!(monster_names("Rat", 2, &state), ["Rat 1", "Rat 2"]);
        for name in ["Goblin", "Goblin 4", "Goblin Boss"] {
            state.apply(&CombatEvent::CombatantAdded {
                combatant: Combatant::new(name, 7),
            });
        }
        assert_eq!(monster_names("Goblin", 2, &state), ["Goblin 5", "Goblin 6"]);
        assert_eq!(monster_names("Goblin Boss", 1, &state), ["Goblin Boss 2"]);
    }
}
