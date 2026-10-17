//! The combat log in words.

use std::collections::{HashMap, HashSet};

use campaign_types::{
    HitPoints,
    combat::{CombatEvent, EffectEndReason, LoggedEvent, SpellSlot, Vitality},
};
use uuid::Uuid;

/// Every combatant and effect that has appeared in the log, by id, so
/// events can name them even after they're gone: what they're called, and
/// which combatants' hit points are behind the DM's screen.
#[derive(Default)]
pub struct Names {
    names: HashMap<Uuid, String>,
    concealed: HashSet<Uuid>,
}

impl Names {
    pub fn of(log: &[LoggedEvent]) -> Self {
        let mut names = Self::default();
        for event in log {
            names.learn(&event.event);
        }
        names
    }

    /// Remembers who an event introduces.
    pub fn learn(&mut self, event: &CombatEvent) {
        match event {
            CombatEvent::CombatantAdded { combatant }
            | CombatEvent::CombatantRestored { combatant } => {
                self.names.insert(combatant.id, combatant.name.clone());
                if combatant.concealed {
                    self.concealed.insert(combatant.id);
                }
            }
            CombatEvent::EffectAdded { effect } | CombatEvent::EffectRestored { effect } => {
                self.names.insert(effect.id, effect.name.clone());
            }
            _ => {}
        }
    }

    fn name(&self, id: &Uuid) -> &str {
        self.names.get(id).map_or("someone", String::as_str)
    }

    /// `id`'s hit points as this device may see them: "12/18", or how hurt
    /// they look.
    fn hit_points(&self, id: &Uuid, hp: HitPoints) -> String {
        if self.concealed.contains(id) {
            Vitality::from_concealed(hp).to_string()
        } else {
            format!("{}/{}", hp.current, hp.max)
        }
    }
}

/// `event` in words; an undo names what it took back, found in `log`.
pub fn describe_in(event: &CombatEvent, log: &[LoggedEvent], names: &Names) -> String {
    if let CombatEvent::Undone { command } = event
        && let Some(first) = log.iter().find(|e| e.command_id == Some(*command))
    {
        return format!("Undo: {}", describe(&first.event, names));
    }
    describe(event, names)
}

pub fn describe(event: &CombatEvent, names: &Names) -> String {
    let name = |id: &Uuid| names.name(id);
    let list = |ids: &[Uuid]| ids.iter().map(name).collect::<Vec<_>>().join(", ");
    match event {
        CombatEvent::Created { name, .. } => format!("{name} is set up"),
        CombatEvent::CombatantAdded { combatant } if combatant.concealed => {
            format!("{} joins", combatant.name)
        }
        CombatEvent::CombatantAdded { combatant } => format!(
            "{} joins ({}/{} HP)",
            combatant.name, combatant.hit_points.current, combatant.hit_points.max
        ),
        CombatEvent::CombatantRemoved { combatant } => format!("{} leaves", name(combatant)),
        CombatEvent::InitiativeRolled {
            combatant,
            roll,
            modifier,
            total,
        } => format!(
            "{} rolls {roll}{modifier:+} = {total} for initiative",
            name(combatant)
        ),
        CombatEvent::InitiativeSet { combatant, total } => {
            format!("{}'s initiative is {total}", name(combatant))
        }
        CombatEvent::Started => "The combat starts".into(),
        CombatEvent::RoundStarted { round } => format!("── Round {round} ──"),
        CombatEvent::TurnStarted { combatant } => format!("{}'s turn", name(combatant)),
        CombatEvent::Damaged {
            combatant,
            amount,
            hit_points: hp,
        } => {
            let down = match hp.current {
                0 if !names.concealed.contains(combatant) => ", down",
                _ => "",
            };
            format!(
                "{} takes {amount} damage ({}{down})",
                name(combatant),
                names.hit_points(combatant, *hp)
            )
        }
        CombatEvent::Healed {
            combatant,
            amount,
            hit_points: hp,
        } => format!(
            "{} regains {amount} HP ({})",
            name(combatant),
            names.hit_points(combatant, *hp)
        ),
        CombatEvent::TemporaryHitPointsSet { combatant, .. }
            if names.concealed.contains(combatant) =>
        {
            format!("{} gains temporary HP", name(combatant))
        }
        CombatEvent::TemporaryHitPointsSet {
            combatant,
            hit_points,
        } => format!(
            "{} has {} temporary HP",
            name(combatant),
            hit_points.temporary
        ),
        CombatEvent::EffectAdded { effect } => {
            let mut text = effect.name.clone();
            if effect.concentration && effect.source.is_some() {
                // Following the cast that started it.
                let source = effect.source.as_ref().map_or("someone", name);
                return format!("  {source} concentrates on {}", effect.name);
            }
            if let Some(source) = &effect.source {
                text = format!("{}: {text}", name(source));
            }
            if !effect.targets.is_empty() {
                text += &format!(" on {}", list(&effect.targets));
            }
            if effect.concentration {
                text += " (concentration)";
            }
            if let Some(expiry) = effect.expires {
                let turn = expiry
                    .at_turn_of
                    .map_or(String::new(), |id| format!("{}'s turn, ", name(&id)));
                text += &format!(" until {turn}round {}", expiry.round);
            }
            text
        }
        CombatEvent::EffectEnded { effect, reason } => {
            let why = match reason {
                EffectEndReason::Removed => "",
                EffectEndReason::Expired => " (expired)",
                EffectEndReason::ConcentrationReplaced => " (concentrating on something else)",
                EffectEndReason::ConcentrationBroken => " (concentration broken)",
                EffectEndReason::SourceRemoved => " (its source left)",
                EffectEndReason::Undone => " (undone)",
            };
            format!("{} ends{why}", name(effect))
        }
        CombatEvent::SpellCast {
            caster,
            spell,
            slot,
            targets,
        } => {
            let mut text = format!("{} casts {spell}", name(caster));
            if !targets.is_empty() {
                text += &format!(" on {}", list(targets));
            }
            match slot {
                Some(SpellSlot::Level(level)) => {
                    text += &format!(" ({} level)", crate::display::ordinal(*level));
                }
                Some(SpellSlot::Pact(_)) => text += " (pact slot)",
                None => {}
            }
            text
        }
        CombatEvent::FeatureUsed {
            combatant,
            feature,
            amount,
        } => match amount {
            1 => format!("{} uses {feature}", name(combatant)),
            n => format!("{} uses {feature} ×{n}", name(combatant)),
        },
        CombatEvent::Paused => "The combat pauses".into(),
        CombatEvent::Resumed => "The combat resumes".into(),
        CombatEvent::Ended => "The combat ends".into(),
        CombatEvent::Undone { .. } => "Undo:".into(),
        CombatEvent::CombatantRestored { combatant } if combatant.concealed => format!(
            "  {} back, looking {}",
            combatant.name,
            combatant.vitality()
        ),
        CombatEvent::CombatantRestored { combatant } => format!(
            "  {} back to {}/{} HP",
            combatant.name, combatant.hit_points.current, combatant.hit_points.max
        ),
        CombatEvent::EffectRestored { effect } => format!("  {} back in play", effect.name),
        CombatEvent::ProgressRestored {
            round,
            current_turn,
            ..
        } => match current_turn {
            Some(turn) => format!("  back to round {round}, {}'s turn", name(turn)),
            None => format!("  back to round {round}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use campaign_types::combat::{Combatant, Concealer, Effect, Expiry};
    use chrono::Utc;

    use super::*;

    fn logged(seq: u64, command: Option<Uuid>, event: CombatEvent) -> LoggedEvent {
        LoggedEvent {
            seq,
            command_id: command,
            actor: None,
            at: Utc::now(),
            event,
        }
    }

    fn hp(current: u16, max: u16) -> HitPoints {
        HitPoints {
            max,
            current,
            temporary: 0,
        }
    }

    /// Vex (a character) and a goblin, as the DM's log would read them, or
    /// a player's, with the goblin concealed.
    fn fight(as_player: bool) -> Vec<String> {
        let mut vex = Combatant::new("Vex", 30);
        vex.character_id = Some(Uuid::new_v4());
        let goblin = Combatant::new("Goblin", 12);
        let (marked, cast) = (Uuid::new_v4(), Uuid::new_v4());
        let events = [
            CombatEvent::CombatantAdded {
                combatant: vex.clone(),
            },
            CombatEvent::CombatantAdded {
                combatant: goblin.clone(),
            },
            CombatEvent::InitiativeRolled {
                combatant: vex.id,
                roll: 14,
                modifier: 3,
                total: 17,
            },
            CombatEvent::TurnStarted { combatant: vex.id },
            CombatEvent::SpellCast {
                caster: vex.id,
                spell: "Hunter's Mark".into(),
                slot: Some(SpellSlot::Level(1)),
                targets: vec![goblin.id],
            },
            CombatEvent::EffectAdded {
                effect: Effect {
                    id: marked,
                    name: "Hunter's Mark".into(),
                    source: Some(vex.id),
                    targets: vec![goblin.id],
                    concentration: true,
                    expires: None,
                },
            },
            CombatEvent::Damaged {
                combatant: goblin.id,
                amount: 7,
                hit_points: hp(5, 12),
            },
            CombatEvent::FeatureUsed {
                combatant: vex.id,
                feature: "Favored Foe".into(),
                amount: 2,
            },
            CombatEvent::EffectAdded {
                effect: Effect {
                    id: Uuid::new_v4(),
                    name: "Prone".into(),
                    source: None,
                    targets: vec![goblin.id],
                    concentration: false,
                    expires: Some(Expiry {
                        round: 2,
                        at_turn_of: Some(vex.id),
                    }),
                },
            },
            CombatEvent::EffectEnded {
                effect: marked,
                reason: EffectEndReason::ConcentrationBroken,
            },
            CombatEvent::Damaged {
                combatant: goblin.id,
                amount: 5,
                hit_points: hp(0, 12),
            },
            CombatEvent::CombatantRemoved {
                combatant: goblin.id,
            },
            CombatEvent::Undone { command: cast },
            CombatEvent::CombatantRestored {
                combatant: Combatant {
                    hit_points: hp(5, 12),
                    ..goblin
                },
            },
            CombatEvent::Ended,
        ];
        // The removal and its undo are one command.
        let log: Vec<LoggedEvent> = events
            .into_iter()
            .zip(1..)
            .map(|(event, seq)| {
                let command = matches!(event, CombatEvent::CombatantRemoved { .. }).then_some(cast);
                logged(seq, command, event)
            })
            .collect();
        let log: Vec<LoggedEvent> = if as_player {
            let mut concealer = Concealer::new(&Default::default());
            log.iter().map(|e| concealer.event(e)).collect()
        } else {
            log
        };
        let names = Names::of(&log);
        log.iter()
            .map(|e| describe_in(&e.event, &log, &names))
            .collect()
    }

    #[test]
    fn the_log_reads_as_what_happened() {
        assert_eq!(
            fight(false),
            [
                "Vex joins (30/30 HP)",
                "Goblin joins (12/12 HP)",
                "Vex rolls 14+3 = 17 for initiative",
                "Vex's turn",
                "Vex casts Hunter's Mark on Goblin (1st level)",
                "  Vex concentrates on Hunter's Mark",
                "Goblin takes 7 damage (5/12)",
                "Vex uses Favored Foe ×2",
                "Prone on Goblin until Vex's turn, round 2",
                "Hunter's Mark ends (concentration broken)",
                "Goblin takes 5 damage (0/12, down)",
                // Named after leaving, too.
                "Goblin leaves",
                "Undo: Goblin leaves",
                "  Goblin back to 5/12 HP",
                "The combat ends",
            ]
        );
    }

    #[test]
    fn a_player_reads_how_hurt_a_monster_looks_not_its_numbers() {
        let log = fight(true);
        assert_eq!(log[0], "Vex joins (30/30 HP)", "the party's own numbers");
        assert_eq!(log[1], "Goblin joins");
        assert_eq!(log[6], "Goblin takes 7 damage (bloodied)");
        assert_eq!(log[10], "Goblin takes 5 damage (down)");
        assert_eq!(log[13], "  Goblin back, looking bloodied");
    }
}
