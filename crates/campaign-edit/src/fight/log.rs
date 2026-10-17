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
