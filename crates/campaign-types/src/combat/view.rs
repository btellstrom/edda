//! What a player's device sees of a combat. The party knows each other's
//! hit points, but monsters' (and anyone else the DM adds) stay behind the
//! DM's screen: a player sees only how hurt one looks ([`Vitality`]), and
//! not its armor class.
//!
//! The combat service conceals the snapshot, the log and every event it
//! sends to a player's device ([`Concealer`]). Concealed hit points still
//! go up and down in steps (3 unhurt, 2 hurt, 1 bloodied, 0 down), so a
//! client applies concealed events like any others, and "is it down?"
//! means the same.

use std::{collections::HashSet, fmt};

use uuid::Uuid;

use super::{CombatEvent, CombatState, Combatant, LoggedEvent};
use crate::HitPoints;

/// How hurt a combatant looks, without the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Vitality {
    Down,
    /// At half its hit points or fewer.
    Bloodied,
    Hurt,
    Unhurt,
}

impl Vitality {
    /// How hurt someone with these hit points looks.
    pub fn of(hp: HitPoints) -> Self {
        match hp.current {
            0 => Vitality::Down,
            current if u32::from(current) * 2 <= u32::from(hp.max) => Vitality::Bloodied,
            current if current < hp.max => Vitality::Hurt,
            _ => Vitality::Unhurt,
        }
    }

    /// Concealed hit points: only this, in steps.
    pub fn hit_points(self) -> HitPoints {
        HitPoints {
            max: 3,
            current: self as u16,
            temporary: 0,
        }
    }

    /// What concealed hit points say.
    pub fn from_concealed(hp: HitPoints) -> Self {
        match hp.current {
            0 => Vitality::Down,
            1 => Vitality::Bloodied,
            2 => Vitality::Hurt,
            _ => Vitality::Unhurt,
        }
    }
}

impl fmt::Display for Vitality {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Vitality::Down => "down",
            Vitality::Bloodied => "bloodied",
            Vitality::Hurt => "hurt",
            Vitality::Unhurt => "unhurt",
        })
    }
}

/// Whether a player's device sees only how hurt `combatant` looks: anyone
/// who isn't a party character.
fn behind_the_screen(combatant: &Combatant) -> bool {
    combatant.character_id.is_none()
}

fn conceal(combatant: &mut Combatant) {
    if behind_the_screen(combatant) && !combatant.concealed {
        combatant.hit_points = Vitality::of(combatant.hit_points).hit_points();
        combatant.armor_class = None;
        combatant.concealed = true;
    }
}

/// Conceals a combat for a player's device: its state, then the events
/// that follow it. It remembers who's behind the screen, so an event about
/// a combatant who has since left is concealed too.
pub struct Concealer {
    hidden: HashSet<Uuid>,
}

impl Concealer {
    /// For a combat as it stands in `state` (not concealed yet).
    pub fn new(state: &CombatState) -> Self {
        let mut concealer = Self {
            hidden: HashSet::new(),
        };
        concealer.learn(state.combatants.iter());
        concealer
    }

    fn learn<'c>(&mut self, combatants: impl Iterator<Item = &'c Combatant>) {
        self.hidden
            .extend(combatants.filter(|c| behind_the_screen(c)).map(|c| c.id));
    }

    /// `state` as a player's device sees it.
    pub fn state(&self, state: &CombatState) -> CombatState {
        let mut state = state.clone();
        state.combatants.iter_mut().for_each(conceal);
        state
    }

    /// `event` as a player's device sees it.
    pub fn event(&mut self, event: &LoggedEvent) -> LoggedEvent {
        let mut event = event.clone();
        match &mut event.event {
            CombatEvent::CombatantAdded { combatant }
            | CombatEvent::CombatantRestored { combatant } => {
                self.learn(std::iter::once(&*combatant));
                conceal(combatant);
            }
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
            } if self.hidden.contains(combatant) => {
                *hit_points = Vitality::of(*hit_points).hit_points();
            }
            _ => {}
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn logged(seq: u64, event: CombatEvent) -> LoggedEvent {
        LoggedEvent {
            seq,
            command_id: None,
            actor: None,
            at: Utc::now(),
            event,
        }
    }

    #[test]
    fn players_see_how_hurt_monsters_look_and_everything_about_the_party() {
        let mut goblin = Combatant::new("Goblin", 12);
        goblin.armor_class = Some(15);
        let mut vex = Combatant::new("Vex", 30);
        vex.character_id = Some(Uuid::new_v4());
        let mut state = CombatState::default();
        state.apply(&CombatEvent::CombatantAdded {
            combatant: vex.clone(),
        });
        let mut concealer = Concealer::new(&state);
        let mut seen = concealer.state(&state);

        let hit = |current| HitPoints {
            max: 12,
            current,
            temporary: 0,
        };
        let events = [
            CombatEvent::CombatantAdded {
                combatant: goblin.clone(),
            },
            CombatEvent::Damaged {
                combatant: goblin.id,
                amount: 3,
                hit_points: hit(9),
            },
            CombatEvent::Damaged {
                combatant: goblin.id,
                amount: 3,
                hit_points: hit(6),
            },
            CombatEvent::Damaged {
                combatant: vex.id,
                amount: 4,
                hit_points: HitPoints {
                    max: 30,
                    current: 26,
                    temporary: 0,
                },
            },
            CombatEvent::CombatantRemoved {
                combatant: goblin.id,
            },
            CombatEvent::CombatantRestored {
                combatant: Combatant {
                    hit_points: hit(1),
                    ..goblin.clone()
                },
            },
            CombatEvent::Damaged {
                combatant: goblin.id,
                amount: 1,
                hit_points: hit(0),
            },
        ];
        let mut looks = Vec::new();
        for (seq, event) in events.into_iter().enumerate() {
            let shown = concealer.event(&logged(seq as u64, event));
            seen.apply(&shown.event);
            if let Some(g) = seen.combatant(goblin.id) {
                assert!(g.concealed && g.armor_class.is_none());
                looks.push(g.vitality());
            }
        }
        use Vitality::*;
        assert_eq!(looks, [Unhurt, Hurt, Bloodied, Bloodied, Bloodied, Down]);
        assert!(seen.combatant(goblin.id).unwrap().is_down());
        let vex_seen = seen.combatant(vex.id).unwrap();
        assert!(!vex_seen.concealed);
        assert_eq!(vex_seen.hit_points.current, 26);
    }

    #[test]
    fn bloodied_is_half_or_fewer() {
        let hp = |current, max| HitPoints {
            max,
            current,
            temporary: 0,
        };
        assert_eq!(Vitality::of(hp(7, 7)), Vitality::Unhurt);
        assert_eq!(Vitality::of(hp(4, 7)), Vitality::Hurt);
        assert_eq!(Vitality::of(hp(3, 7)), Vitality::Bloodied);
        assert_eq!(Vitality::of(hp(0, 7)), Vitality::Down);
        for v in [
            Vitality::Down,
            Vitality::Bloodied,
            Vitality::Hurt,
            Vitality::Unhurt,
        ] {
            assert_eq!(Vitality::from_concealed(v.hit_points()), v);
        }
    }
}
