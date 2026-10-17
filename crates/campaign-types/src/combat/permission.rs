//! What a player's device may do in a combat. The DM's devices may do
//! anything. A player changes only their own combatants (those playing a
//! character the device owns); whatever happens to anyone else, the DM or
//! that combatant's player enters themselves. So a player may:
//!
//! - damage, heal and give temporary hit points to their own combatants,
//!   and use their features;
//! - cast spells and put down effects that name only their own combatants,
//!   and end effects that involve only them;
//! - concentrate on a spell or effect of their own on anyone (Bless on the
//!   party, Hunter's Mark on a goblin), and end it: it only marks whom it's
//!   on, and ends with their concentration;
//! - end their own turn;
//! - undo their own commands.
//!
//! Everything else (who's in the fight, initiative, starting, pausing and
//! ending it) is the DM's. The combat service enforces this; clients can
//! check it first to say so before sending.

use uuid::Uuid;

use super::{CombatCommand, CombatError, CombatState, Combatant};

/// Whether a player may send `command`, given which combatants are their
/// own. Combatants or effects the combat doesn't have are let through, for
/// [`super::decide`] to refuse.
pub fn player_may(
    state: &CombatState,
    command: &CombatCommand,
    own: impl Fn(&Combatant) -> bool,
) -> Result<(), CombatError> {
    let mine = |ids: &mut dyn Iterator<Item = &Uuid>| -> Result<(), CombatError> {
        for id in ids {
            if let Some(combatant) = state.combatant(*id)
                && !own(combatant)
            {
                return Err(CombatError::NotYours(combatant.name.clone()));
            }
        }
        Ok(())
    };
    match command {
        CombatCommand::Damage { combatant, .. }
        | CombatCommand::Heal { combatant, .. }
        | CombatCommand::SetTemporaryHitPoints { combatant, .. }
        | CombatCommand::UseFeature { combatant, .. } => mine(&mut std::iter::once(combatant)),
        // Whom a concentration spell or effect of one's own is on is only
        // marked.
        CombatCommand::CastSpell {
            caster,
            concentration: Some(_),
            ..
        } => mine(&mut std::iter::once(caster)),
        CombatCommand::CastSpell {
            caster, targets, ..
        } => mine(&mut std::iter::once(caster).chain(targets)),
        CombatCommand::AddEffect { effect } => match effect.source {
            Some(source) if effect.concentration => mine(&mut std::iter::once(&source)),
            _ => mine(&mut effect.source.iter().chain(&effect.targets)),
        },
        CombatCommand::RemoveEffect { effect } => match state.effect(*effect) {
            Some(effect) => match effect.source {
                Some(source) if effect.concentration => mine(&mut std::iter::once(&source)),
                _ => mine(&mut effect.source.iter().chain(&effect.targets)),
            },
            None => Ok(()),
        },
        CombatCommand::NextTurn => match state.current_turn.and_then(|id| state.combatant(id)) {
            Some(current) if own(current) => Ok(()),
            _ => Err(CombatError::NotYourTurn),
        },
        // Only their own, which the server checks against the log.
        CombatCommand::Undo { .. } => Ok(()),
        CombatCommand::AddCombatant { .. }
        | CombatCommand::AddMonsters { .. }
        | CombatCommand::RemoveCombatant { .. }
        | CombatCommand::RollInitiative { .. }
        | CombatCommand::RollInitiativeForAll
        | CombatCommand::SetInitiative { .. }
        | CombatCommand::Start
        | CombatCommand::Pause
        | CombatCommand::Resume
        | CombatCommand::End => Err(CombatError::DmOnly),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::{CombatEvent, NewEffect};

    /// Vex (the player's) has the turn; Grog (another player's) and a
    /// goblin are in the fight too.
    fn fight() -> (CombatState, [Uuid; 3]) {
        let mut state = CombatState::default();
        let mut ids = [Uuid::nil(); 3];
        for (i, name) in ["Vex", "Grog", "Goblin"].into_iter().enumerate() {
            let mut combatant = Combatant::new(name, 10);
            combatant.initiative = Some(20 - i16::try_from(i).unwrap());
            ids[i] = combatant.id;
            state.apply(&CombatEvent::CombatantAdded { combatant });
        }
        state.apply(&CombatEvent::Started);
        state.apply(&CombatEvent::TurnStarted { combatant: ids[0] });
        (state, ids)
    }

    fn vex_may(state: &CombatState, command: CombatCommand) -> Result<(), CombatError> {
        player_may(state, &command, |c| c.name == "Vex")
    }

    #[test]
    fn players_change_only_their_own_combatants() {
        let (state, [vex, grog, goblin]) = fight();
        let damage = |combatant| CombatCommand::Damage {
            combatant,
            amount: 3,
        };
        assert_eq!(vex_may(&state, damage(vex)), Ok(()));
        assert_eq!(
            vex_may(&state, damage(goblin)),
            Err(CombatError::NotYours("Goblin".into()))
        );
        let cast = |targets| CombatCommand::CastSpell {
            caster: vex,
            spell: "Cure Wounds".into(),
            slot: None,
            targets,
            concentration: None,
        };
        assert_eq!(vex_may(&state, cast(vec![vex])), Ok(()));
        assert_eq!(
            vex_may(&state, cast(vec![vex, grog])),
            Err(CombatError::NotYours("Grog".into()))
        );
        let effect = |source, targets| CombatCommand::AddEffect {
            effect: NewEffect {
                id: Uuid::new_v4(),
                name: "Blessed".into(),
                source,
                targets,
                concentration: false,
                rounds: None,
            },
        };
        assert_eq!(vex_may(&state, effect(None, vec![vex])), Ok(()));
        assert!(vex_may(&state, effect(Some(vex), vec![goblin])).is_err());
        assert!(vex_may(&state, effect(Some(grog), vec![vex])).is_err());
    }

    #[test]
    fn players_concentrate_on_anyone_but_only_for_themselves() {
        let (mut state, [vex, grog, goblin]) = fight();
        let concentrate = |source, targets| CombatCommand::AddEffect {
            effect: NewEffect {
                id: Uuid::new_v4(),
                name: "Hunter's Mark".into(),
                source,
                targets,
                concentration: true,
                rounds: None,
            },
        };
        assert_eq!(
            vex_may(&state, concentrate(Some(vex), vec![goblin])),
            Ok(())
        );
        assert!(vex_may(&state, concentrate(Some(grog), vec![goblin])).is_err());
        assert!(vex_may(&state, concentrate(None, vec![goblin])).is_err());
        let bless = |concentration| CombatCommand::CastSpell {
            caster: vex,
            spell: "Bless".into(),
            slot: None,
            targets: vec![vex, grog],
            concentration,
        };
        assert_eq!(vex_may(&state, bless(Some(Uuid::new_v4()))), Ok(()));
        assert_eq!(
            vex_may(&state, bless(None)),
            Err(CombatError::NotYours("Grog".into()))
        );

        // Ending it is hers too; ending Grog's isn't.
        let mark = |id, source| CombatEvent::EffectAdded {
            effect: crate::combat::Effect {
                id,
                name: "Mark".into(),
                source: Some(source),
                targets: vec![goblin],
                concentration: true,
                expires: None,
            },
        };
        let (hers, his) = (Uuid::new_v4(), Uuid::new_v4());
        state.apply(&mark(hers, vex));
        state.apply(&mark(his, grog));
        let drop = |effect| CombatCommand::RemoveEffect { effect };
        assert_eq!(vex_may(&state, drop(hers)), Ok(()));
        assert!(vex_may(&state, drop(his)).is_err());
    }

    #[test]
    fn players_end_only_their_own_turn_and_the_dm_runs_the_rest() {
        let (state, [vex, ..]) = fight();
        assert_eq!(vex_may(&state, CombatCommand::NextTurn), Ok(()));
        let grogs_turn = |c: &Combatant| c.name == "Grog";
        assert_eq!(
            player_may(&state, &CombatCommand::NextTurn, grogs_turn),
            Err(CombatError::NotYourTurn)
        );
        for command in [
            CombatCommand::Start,
            CombatCommand::End,
            CombatCommand::RemoveCombatant { combatant: vex },
            CombatCommand::RollInitiative {
                combatant: vex,
                roll: None,
            },
        ] {
            assert_eq!(vex_may(&state, command), Err(CombatError::DmOnly));
        }
        assert_eq!(
            vex_may(&state, CombatCommand::Undo { command: None }),
            Ok(())
        );
    }
}
