//! The combat log in words.

use std::collections::HashMap;

use campaign_types::combat::{CombatEvent, EffectEndReason, LoggedEvent, SpellSlot};
use uuid::Uuid;

/// Names of every combatant and effect that appears in `log`.
pub fn names(log: &[LoggedEvent]) -> HashMap<Uuid, String> {
    let mut names = HashMap::new();
    for event in log {
        learn(&mut names, &event.event);
    }
    names
}

/// Remembers the names an event introduces, so later events can use them
/// even after the combatant or effect is gone.
pub fn learn(names: &mut HashMap<Uuid, String>, event: &CombatEvent) {
    match event {
        CombatEvent::CombatantAdded { combatant }
        | CombatEvent::CombatantRestored { combatant } => {
            names.insert(combatant.id, combatant.name.clone());
        }
        CombatEvent::EffectAdded { effect } | CombatEvent::EffectRestored { effect } => {
            names.insert(effect.id, effect.name.clone());
        }
        _ => {}
    }
}

/// `event` in words; an undo names what it took back, found in `log`.
pub fn describe_in(
    event: &CombatEvent,
    log: &[LoggedEvent],
    names: &HashMap<Uuid, String>,
) -> String {
    if let CombatEvent::Undone { command } = event
        && let Some(first) = log.iter().find(|e| e.command_id == Some(*command))
    {
        return format!("Undo: {}", describe(&first.event, names));
    }
    describe(event, names)
}

pub fn describe(event: &CombatEvent, names: &HashMap<Uuid, String>) -> String {
    let name = |id: &Uuid| names.get(id).map_or("someone", String::as_str);
    let list = |ids: &[Uuid]| ids.iter().map(name).collect::<Vec<_>>().join(", ");
    match event {
        CombatEvent::Created { name, .. } => format!("{name} is set up"),
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
            let down = if hp.current == 0 { ", down" } else { "" };
            format!(
                "{} takes {amount} damage ({}/{}{down})",
                name(combatant),
                hp.current,
                hp.max
            )
        }
        CombatEvent::Healed {
            combatant,
            amount,
            hit_points: hp,
        } => format!(
            "{} regains {amount} HP ({}/{})",
            name(combatant),
            hp.current,
            hp.max
        ),
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
