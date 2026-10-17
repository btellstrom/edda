//! Undoing a command.
//!
//! The log is append-only, so an undo never removes anything: it appends an
//! [`CombatEvent::Undone`] marker followed by events that put back what the
//! undone command changed. Everyone sees the correction like any other
//! change, and the log still shows both the mistake and its undo.
//!
//! Later commands by other people are kept. Only what the undone command
//! itself changed is reverted:
//!
//! - Hit points move back by the amount the command changed them, so 7
//!   damage undone after someone else's 2 leaves the 2.
//! - Initiative goes back unless someone has changed it since.
//! - Combatants and effects the command added are taken out again; ones it
//!   removed or ended come back. An effect isn't brought back if its source
//!   has started concentrating on something else since.
//! - Round, turn and status (from `start`, `next_turn`, `pause`, `end`, a
//!   removal passing the turn) go back only if nothing has moved them
//!   since; otherwise the undo is refused, as the turn has moved on.
//!
//! An ended combat accepts only the undo of the command that ended it.
//!
//! Spells and features are spent on character sheets, outside the combat:
//! undoing a cast ends the effect it put into play, and the combat service
//! gives the slot back to the sheet.

use std::collections::HashSet;

use uuid::Uuid;

use super::{
    CombatError, CombatEvent, CombatState, CombatStatus, Combatant, EffectEndReason, LoggedEvent,
    rules::Emitter,
};

/// The events that undo `target` (or, if `None`, the latest command for
/// which `mine` holds that isn't undone yet), given the combat's full log.
/// `mine` says whether a command was sent by whoever is undoing.
pub fn undo(
    log: &[LoggedEvent],
    target: Option<Uuid>,
    mine: impl Fn(Uuid) -> bool,
) -> Result<Vec<CombatEvent>, CombatError> {
    let target = which(log, target, mine)?;
    let [before, after, now] = around(log, target);

    let progress = |s: &CombatState| (s.status, s.round, s.current_turn);
    let rewind = progress(&before) != progress(&after);
    if rewind && progress(&now) != progress(&after) {
        return Err(CombatError::CannotUndo(
            "the combat has moved on since".into(),
        ));
    }
    if now.status == CombatStatus::Ended && !rewind {
        return Err(CombatError::Ended);
    }

    let mut out = Emitter {
        state: now,
        events: Vec::new(),
    };
    out.emit(CombatEvent::Undone { command: target });
    out.restore_combatants(&before, &after)?;
    // Ended before any are restored, so a concentration effect the command
    // replaced can come back.
    out.end_added_effects(&before, &after);
    out.restore_effects(&before, &after);
    out.remove_added_combatants(&before, &after)?;
    if rewind {
        out.restore_progress(&before)?;
    }
    Ok(out.events)
}

/// The command to undo: `target`, if it can be undone by whoever `mine`
/// says sent it, or their latest one not undone yet.
fn which(
    log: &[LoggedEvent],
    target: Option<Uuid>,
    mine: impl Fn(Uuid) -> bool,
) -> Result<Uuid, CombatError> {
    let undone: HashSet<Uuid> = log
        .iter()
        .filter_map(|e| match e.event {
            CombatEvent::Undone { command } => Some(command),
            _ => None,
        })
        .collect();
    // The undos themselves, which can't be undone in turn.
    let undos: HashSet<Uuid> = log
        .iter()
        .filter(|e| matches!(e.event, CombatEvent::Undone { .. }))
        .filter_map(|e| e.command_id)
        .collect();

    let Some(command) = target else {
        return log
            .iter()
            .rev()
            .filter_map(|e| e.command_id)
            .find(|c| !undos.contains(c) && !undone.contains(c) && mine(*c))
            .ok_or(CombatError::NothingToUndo);
    };
    if !log.iter().any(|e| e.command_id == Some(command)) {
        return Err(CombatError::UnknownCommand(command));
    }
    if undos.contains(&command) {
        return Err(CombatError::CannotUndo("an undo can't be undone".into()));
    }
    if !mine(command) {
        return Err(CombatError::NotYourCommand);
    }
    if undone.contains(&command) {
        return Err(CombatError::AlreadyUndone);
    }
    Ok(command)
}

/// The combat just before `command`, just after it, and now.
fn around(log: &[LoggedEvent], command: Uuid) -> [CombatState; 3] {
    // A command's events are contiguous: commands to a combat are applied
    // one at a time.
    let first = log
        .iter()
        .position(|e| e.command_id == Some(command))
        .expect("the command is in the log");
    let last = log
        .iter()
        .rposition(|e| e.command_id == Some(command))
        .expect("the command is in the log");
    let replay = |events: &[LoggedEvent]| CombatState::replay(events.iter().map(|e| &e.event));
    [replay(&log[..first]), replay(&log[..=last]), replay(log)]
}

/// Taking back what the command changed, from the combat as it was
/// `before` the command and `after` it.
impl Emitter {
    /// Combatants it removed or changed.
    fn restore_combatants(
        &mut self,
        before: &CombatState,
        after: &CombatState,
    ) -> Result<(), CombatError> {
        for old in &before.combatants {
            let current = self.state.combatant(old.id).cloned();
            match (after.combatant(old.id), current) {
                (None, None) => self.emit(CombatEvent::CombatantRestored {
                    combatant: old.clone(),
                }),
                (Some(new), Some(current)) => {
                    let reverted = revert(old, new, &current);
                    if reverted != current {
                        self.emit(CombatEvent::CombatantRestored {
                            combatant: reverted,
                        });
                    }
                }
                (Some(new), None) if new != old => {
                    return Err(CombatError::CannotUndo(format!(
                        "{} has left the combat",
                        old.name
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Effects it added.
    fn end_added_effects(&mut self, before: &CombatState, after: &CombatState) {
        for effect in &after.effects {
            if before.effect(effect.id).is_none() && self.state.effect(effect.id).is_some() {
                self.emit(CombatEvent::EffectEnded {
                    effect: effect.id,
                    reason: EffectEndReason::Undone,
                });
            }
        }
    }

    /// Effects it ended, or whose targets it removed.
    fn restore_effects(&mut self, before: &CombatState, after: &CombatState) {
        let present = |state: &CombatState, id: &Uuid| state.combatant(*id).is_some();
        for old in &before.effects {
            match (after.effect(old.id), self.state.effect(old.id).cloned()) {
                (None, None) => {
                    let replaced = old.concentration
                        && self
                            .state
                            .effects
                            .iter()
                            .any(|e| e.concentration && e.source == old.source);
                    if !replaced {
                        let mut effect = old.clone();
                        effect.targets.retain(|t| present(&self.state, t));
                        self.emit(CombatEvent::EffectRestored { effect });
                    }
                }
                (Some(new), Some(mut current)) => {
                    let lost = old.targets.iter().filter(|t| !new.targets.contains(t));
                    let back: Vec<_> = lost
                        .filter(|t| present(&self.state, t) && !current.targets.contains(t))
                        .copied()
                        .collect();
                    if !back.is_empty() {
                        current.targets.extend(back);
                        self.emit(CombatEvent::EffectRestored { effect: current });
                    }
                }
                _ => {}
            }
        }
    }

    /// Combatants it added.
    fn remove_added_combatants(
        &mut self,
        before: &CombatState,
        after: &CombatState,
    ) -> Result<(), CombatError> {
        for new in &after.combatants {
            if before.combatant(new.id).is_none() && self.state.combatant(new.id).is_some() {
                self.remove(new.id)?;
            }
        }
        Ok(())
    }

    /// The round, turn and status from before it.
    fn restore_progress(&mut self, before: &CombatState) -> Result<(), CombatError> {
        if let Some(turn) = before.current_turn
            && self.state.combatant(turn).is_none()
        {
            return Err(CombatError::CannotUndo(
                "whose turn it was has left the combat".into(),
            ));
        }
        self.emit(CombatEvent::ProgressRestored {
            status: before.status,
            round: before.round,
            current_turn: before.current_turn,
        });
        Ok(())
    }
}

/// `current` with the change from `old` to `new` taken back: hit points
/// move back by the same amount; initiative goes back unless it has been
/// changed again since.
fn revert(old: &Combatant, new: &Combatant, current: &Combatant) -> Combatant {
    let shift = |now: u16, from: u16, to: u16, max: u16| {
        let value = i32::from(now) - (i32::from(to) - i32::from(from));
        u16::try_from(value.clamp(0, i32::from(max))).expect("clamped")
    };
    let (o, n, c) = (old.hit_points, new.hit_points, current.hit_points);
    let mut reverted = current.clone();
    reverted.hit_points.current = shift(c.current, o.current, n.current, c.max);
    reverted.hit_points.temporary = shift(c.temporary, o.temporary, n.temporary, u16::MAX);
    if new.initiative != old.initiative && current.initiative == new.initiative {
        reverted.initiative = old.initiative;
    }
    reverted
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::combat::{CombatCommand, NewEffect, NoDice, decide};

    /// A combat run by several people, recording who sent what.
    struct Table {
        log: Vec<LoggedEvent>,
    }

    impl Table {
        fn new() -> Self {
            let mut table = Self { log: Vec::new() };
            table.push(
                None,
                None,
                CombatEvent::Created {
                    name: "Ambush".into(),
                    campaign: "Test".into(),
                },
            );
            table
        }

        fn push(&mut self, command_id: Option<Uuid>, actor: Option<&str>, event: CombatEvent) {
            self.log.push(LoggedEvent {
                seq: self.log.len() as u64 + 1,
                command_id,
                actor: actor.map(str::to_string),
                at: Utc::now(),
                event,
            });
        }

        fn state(&self) -> CombatState {
            CombatState::replay(self.log.iter().map(|e| &e.event))
        }

        /// Runs `command` as `actor`; returns its command id.
        fn run(&mut self, actor: &str, command: CombatCommand) -> Uuid {
            let events = decide(&self.state(), &command, &mut NoDice).unwrap();
            let id = Uuid::new_v4();
            for event in events {
                self.push(Some(id), Some(actor), event);
            }
            id
        }

        fn try_undo(&mut self, actor: &str, target: Option<Uuid>) -> Result<Uuid, CombatError> {
            let mine = |command| {
                self.log
                    .iter()
                    .any(|e| e.command_id == Some(command) && e.actor.as_deref() == Some(actor))
            };
            let events = undo(&self.log, target, mine)?;
            let id = Uuid::new_v4();
            for event in events {
                self.push(Some(id), Some(actor), event);
            }
            Ok(id)
        }

        fn undo(&mut self, actor: &str) {
            self.try_undo(actor, None).unwrap();
        }

        fn add(&mut self, actor: &str, name: &str, hp: u16) -> Uuid {
            let combatant = Combatant::new(name, hp);
            let id = combatant.id;
            self.run(actor, CombatCommand::AddCombatant { combatant });
            id
        }

        fn hp(&self, id: Uuid) -> u16 {
            self.state().combatant(id).unwrap().hit_points.current
        }

        fn damage(&mut self, actor: &str, combatant: Uuid, amount: u32) -> Uuid {
            self.run(actor, CombatCommand::Damage { combatant, amount })
        }

        fn init(&mut self, actor: &str, combatant: Uuid, total: i16) -> Uuid {
            self.run(actor, CombatCommand::SetInitiative { combatant, total })
        }

        fn effect(&mut self, actor: &str, source: Uuid, name: &str, rounds: Option<u32>) -> Uuid {
            let id = Uuid::new_v4();
            let effect = NewEffect {
                id,
                name: name.into(),
                source: Some(source),
                targets: vec![],
                concentration: true,
                rounds,
            };
            self.run(actor, CombatCommand::AddEffect { effect });
            id
        }

        fn current(&self) -> (u32, String) {
            let state = self.state();
            let turn = state.combatant(state.current_turn.unwrap()).unwrap();
            (state.round, turn.name.clone())
        }
    }

    #[test]
    fn undoing_damage_keeps_damage_dealt_since() {
        let mut t = Table::new();
        let goblin = t.add("DM", "Goblin", 20);
        t.damage("Vex", goblin, 7);
        t.damage("Grog", goblin, 2);
        t.undo("Vex");
        assert_eq!(t.hp(goblin), 18, "Grog's 2 damage stays");
        assert!(matches!(
            t.log[t.log.len() - 2].event,
            CombatEvent::Undone { .. }
        ));
    }

    #[test]
    fn undo_steps_back_through_ones_own_commands_only() {
        let mut t = Table::new();
        let goblin = t.add("DM", "Goblin", 20);
        t.damage("Vex", goblin, 1);
        t.damage("Vex", goblin, 2);
        t.damage("Grog", goblin, 4);
        t.undo("Vex");
        assert_eq!(t.hp(goblin), 15);
        t.undo("Vex");
        assert_eq!(t.hp(goblin), 16);
        assert_eq!(t.try_undo("Vex", None), Err(CombatError::NothingToUndo));
        // The DM's add is still there to undo, but it's the DM's.
        assert_eq!(t.state().combatants.len(), 1);
    }

    #[test]
    fn a_specific_command_can_be_undone_if_it_is_ones_own() {
        let mut t = Table::new();
        let goblin = t.add("DM", "Goblin", 20);
        let first = t.damage("Vex", goblin, 1);
        t.damage("Vex", goblin, 2);
        let groggy = t.damage("Grog", goblin, 4);

        assert_eq!(
            t.try_undo("Vex", Some(groggy)),
            Err(CombatError::NotYourCommand)
        );
        let unknown = Uuid::new_v4();
        assert_eq!(
            t.try_undo("Vex", Some(unknown)),
            Err(CombatError::UnknownCommand(unknown))
        );
        let the_undo = t.try_undo("Vex", Some(first)).unwrap();
        assert_eq!(t.hp(goblin), 14);
        assert_eq!(
            t.try_undo("Vex", Some(first)),
            Err(CombatError::AlreadyUndone)
        );
        assert!(matches!(
            t.try_undo("Vex", Some(the_undo)),
            Err(CombatError::CannotUndo(_))
        ));
    }

    #[test]
    fn undoing_a_turn_rewinds_round_turn_and_expiries() {
        let mut t = Table::new();
        let cleric = t.add("DM", "Cleric", 10);
        let orc = t.add("DM", "Orc", 10);
        t.init("DM", cleric, 10);
        t.init("DM", orc, 5);
        t.run("DM", CombatCommand::Start);
        let bless = t.effect("Cleric", cleric, "Bless", Some(1));
        t.run("DM", CombatCommand::NextTurn); // round 1, orc
        t.run("DM", CombatCommand::NextTurn); // round 2, cleric: bless expires
        assert!(t.state().effect(bless).is_none());

        t.damage("Orc player", cleric, 3); // someone else acts meanwhile
        t.undo("DM");
        assert_eq!(t.current(), (1, "Orc".to_string()));
        assert!(t.state().effect(bless).is_some(), "bless back in play");
        assert_eq!(t.hp(cleric), 7, "the damage since is kept");

        // Once someone else moves the turn on, the DM's earlier turn can't
        // be taken back.
        t.run("Orc player", CombatCommand::NextTurn);
        assert_eq!(
            t.try_undo("DM", None),
            Err(CombatError::CannotUndo(
                "the combat has moved on since".into()
            ))
        );
    }

    #[test]
    fn undoing_a_removal_brings_back_the_combatant_their_effects_and_turn() {
        let mut t = Table::new();
        let keyleth = t.add("DM", "Keyleth", 30);
        let grog = t.add("DM", "Grog", 50);
        t.init("DM", keyleth, 15);
        t.init("DM", grog, 10);
        t.run("DM", CombatCommand::Start);
        let id = Uuid::new_v4();
        let effect = NewEffect {
            id,
            name: "Moonbeam".into(),
            source: Some(keyleth),
            targets: vec![grog],
            concentration: true,
            rounds: None,
        };
        t.run("Keyleth", CombatCommand::AddEffect { effect });
        let goblin = t.add("DM", "Goblin", 7);
        let faerie = Uuid::new_v4();
        let effect = NewEffect {
            id: faerie,
            name: "Faerie Fire".into(),
            source: Some(grog),
            targets: vec![keyleth, goblin],
            concentration: false,
            rounds: None,
        };
        t.run("Grog", CombatCommand::AddEffect { effect });
        let before = t.state();

        t.run("DM", CombatCommand::RemoveCombatant { combatant: keyleth });
        assert_eq!(t.current(), (1, "Grog".to_string()));
        t.undo("DM");
        let after = t.state();
        assert_eq!(after.current_turn, Some(keyleth));
        assert_eq!(after.combatant(keyleth), before.combatant(keyleth));
        assert_eq!(after.effect(id), before.effect(id), "concentration back");
        let targets = &after.effect(faerie).unwrap().targets;
        assert!(targets.contains(&keyleth) && targets.contains(&goblin));
        assert_eq!(
            after.turn_order().map(|c| c.id).collect::<Vec<_>>(),
            [keyleth, grog]
        );
    }

    #[test]
    fn undoing_an_addition_takes_it_out_again() {
        let mut t = Table::new();
        let keyleth = t.add("DM", "Keyleth", 30);
        let moonbeam = t.effect("Keyleth", keyleth, "Moonbeam", None);
        let fire = t.effect("Keyleth", keyleth, "Faerie Fire", None);
        assert!(t.state().effect(moonbeam).is_none(), "replaced");
        t.undo("Keyleth");
        let state = t.state();
        assert!(state.effect(fire).is_none());
        assert!(state.effect(moonbeam).is_some(), "the replaced one is back");

        t.undo("DM");
        assert!(t.state().combatants.is_empty());
        assert!(t.state().effects.is_empty(), "their concentration ends");
    }

    #[test]
    fn concentration_broken_by_damage_comes_back_unless_replaced() {
        let mut t = Table::new();
        let keyleth = t.add("DM", "Keyleth", 10);
        let moonbeam = t.effect("Keyleth", keyleth, "Moonbeam", None);
        t.damage("DM", keyleth, 10);
        assert!(t.state().effect(moonbeam).is_none());
        t.undo("DM");
        assert_eq!(t.hp(keyleth), 10);
        assert!(t.state().effect(moonbeam).is_some());

        t.damage("DM", keyleth, 10);
        t.run(
            "Keyleth",
            CombatCommand::Heal {
                combatant: keyleth,
                amount: 5,
            },
        );
        let bless = t.effect("Keyleth", keyleth, "Bless", None);
        t.undo("DM");
        let state = t.state();
        assert_eq!(t.hp(keyleth), 10, "capped at max");
        assert!(state.effect(moonbeam).is_none(), "Bless replaced it since");
        assert!(state.effect(bless).is_some());
    }

    #[test]
    fn hit_points_of_someone_who_left_cant_be_put_back() {
        let mut t = Table::new();
        let goblin = t.add("DM", "Goblin", 20);
        t.damage("Vex", goblin, 7);
        t.run("DM", CombatCommand::RemoveCombatant { combatant: goblin });
        assert_eq!(
            t.try_undo("Vex", None),
            Err(CombatError::CannotUndo("Goblin has left the combat".into()))
        );
    }

    #[test]
    fn initiative_goes_back_unless_changed_since() {
        let mut t = Table::new();
        let goblin = t.add("DM", "Goblin", 7);
        t.init("DM", goblin, 12);
        t.init("Vex", goblin, 3);
        t.undo("Vex");
        assert_eq!(t.state().combatant(goblin).unwrap().initiative, Some(12));

        t.init("Vex", goblin, 3);
        t.init("DM", goblin, 18);
        t.undo("Vex");
        assert_eq!(t.state().combatant(goblin).unwrap().initiative, Some(18));
    }

    #[test]
    fn an_ended_combat_only_undoes_its_ending() {
        let mut t = Table::new();
        let goblin = t.add("DM", "Goblin", 7);
        t.init("DM", goblin, 12);
        t.run("DM", CombatCommand::Start);
        t.damage("Vex", goblin, 1);
        t.run("DM", CombatCommand::Pause);
        t.run("DM", CombatCommand::End);
        assert_eq!(t.try_undo("Vex", None), Err(CombatError::Ended));
        t.undo("DM");
        assert_eq!(t.state().status, CombatStatus::Paused);
        assert_eq!(t.current(), (1, "Goblin".to_string()));
        t.undo("Vex");
        assert_eq!(t.hp(goblin), 7, "undo works while paused");
    }

    #[test]
    fn undoing_a_cast_ends_its_effect_and_restores_the_one_it_replaced() {
        let mut t = Table::new();
        let keyleth = t.add("DM", "Keyleth", 30);
        let moonbeam = t.effect("Keyleth", keyleth, "Moonbeam", None);
        let bless = Uuid::new_v4();
        t.run(
            "Keyleth",
            CombatCommand::CastSpell {
                caster: keyleth,
                spell: "Bless".into(),
                slot: Some(crate::combat::SpellSlot::Level(1)),
                targets: vec![keyleth],
                concentration: Some(bless),
            },
        );
        t.undo("Keyleth");
        let state = t.state();
        assert!(state.effect(bless).is_none());
        assert!(state.effect(moonbeam).is_some());
    }

    #[test]
    fn decide_leaves_undo_to_the_server() {
        let t = Table::new();
        assert_eq!(
            decide(
                &t.state(),
                &CombatCommand::Undo { command: None },
                &mut NoDice
            ),
            Err(CombatError::ServerOnly)
        );
    }
}
