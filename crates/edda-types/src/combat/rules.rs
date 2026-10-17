use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    CombatEvent, CombatState, CombatStatus, Combatant, Effect, EffectEndReason, Expiry, SpellSlot,
};

/// What a participant wants to happen. Turned into events by [`decide`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CombatCommand {
    /// Join the combat, at any point. If `initiative` is already set the
    /// combatant slots straight into the turn order.
    AddCombatant {
        combatant: Combatant,
    },
    /// Add `count` of a monster from the campaign's monsters, named and
    /// numbered on from any already in the fight ("Goblin 3"), at their
    /// maximum hit points or, with `roll`, rolled. Only the server can
    /// decide this (it needs the stat block and dice); see
    /// [`super::add_monsters`].
    AddMonsters {
        monster: Uuid,
        #[serde(default = "one")]
        count: u16,
        #[serde(default)]
        roll: bool,
    },
    RemoveCombatant {
        combatant: Uuid,
    },
    /// Roll d20 + modifier. Pass `roll` when the player rolled physical
    /// dice; leave it out to have the server roll.
    RollInitiative {
        combatant: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        roll: Option<u8>,
    },
    /// Server rolls for everyone who hasn't got initiative yet.
    RollInitiativeForAll,
    SetInitiative {
        combatant: Uuid,
        total: i16,
    },
    Start,
    NextTurn,
    Damage {
        combatant: Uuid,
        amount: u32,
    },
    Heal {
        combatant: Uuid,
        amount: u32,
    },
    SetTemporaryHitPoints {
        combatant: Uuid,
        amount: u16,
    },
    AddEffect {
        effect: NewEffect,
    },
    RemoveEffect {
        effect: Uuid,
    },
    /// Record a spell cast in the log. The slot is spent on the caster's
    /// character sheet, separately; `slot` says which, for the record.
    /// For a concentration spell, `concentration` is the id of the effect
    /// that puts it into play on its targets (ending the caster's previous
    /// concentration).
    CastSpell {
        caster: Uuid,
        spell: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<SpellSlot>,
        #[serde(default)]
        targets: Vec<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        concentration: Option<Uuid>,
    },
    /// Use a limited feature (Rage, Action Surge...). For a character, the
    /// combat service spends the uses on their sheet first.
    UseFeature {
        combatant: Uuid,
        feature: String,
        #[serde(default = "one")]
        amount: u16,
    },
    /// Interrupt an active combat. While paused, only `Resume` and `End`
    /// are accepted.
    Pause,
    Resume,
    End,
    /// Take back one of the sender's own earlier commands: `command`, or
    /// when that's left out, their latest one not yet undone. Only the
    /// server can decide this (it needs the log); see [`super::undo`].
    Undo {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<Uuid>,
    },
}

fn one() -> u16 {
    1
}

/// An effect for a command to put into play.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NewEffect {
    /// Chosen by the client, like combatant ids.
    pub id: Uuid,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Uuid>,
    #[serde(default)]
    pub targets: Vec<Uuid>,
    #[serde(default)]
    pub concentration: bool,
    /// Duration in rounds, counted in turns of the source, or (with no
    /// source, like a fire on the floor) of whoever's turn it is when it's
    /// added; `None` lasts until removed. Targets are optional too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u32>,
}

/// Why the rules refuse a command.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CombatError {
    #[error("no combatant with id {0}")]
    UnknownCombatant(Uuid),
    #[error("no effect with id {0}")]
    UnknownEffect(Uuid),
    #[error("id {0} is already in use")]
    DuplicateId(Uuid),
    #[error("name must not be empty")]
    EmptyName,
    #[error("current hit points exceed maximum")]
    InvalidHitPoints,
    #[error("a d20 roll must be between 1 and 20, got {0}")]
    InvalidRoll(u8),
    #[error("everyone already has initiative")]
    NothingToRoll,
    #[error("add at least one")]
    NothingToAdd,
    /// A player's command would change someone who isn't theirs.
    #[error("{0} isn't yours; the DM or their player changes them")]
    NotYours(String),
    #[error("it isn't your turn")]
    NotYourTurn,
    #[error("only the DM can do that")]
    DmOnly,
    #[error("nobody has initiative yet")]
    NoInitiative,
    #[error("combat has not started")]
    NotStarted,
    #[error("combat has already started")]
    AlreadyStarted,
    #[error("combat is paused")]
    Paused,
    #[error("combat is not paused")]
    NotPaused,
    #[error("combat has ended")]
    Ended,
    /// Only seen client-side: the outcome depends on a server dice roll.
    #[error("this needs a dice roll from the server")]
    DiceUnavailable,
    /// Only seen client-side: the outcome depends on the server's log.
    #[error("only the server can decide this")]
    ServerOnly,
    #[error("you have nothing left to undo")]
    NothingToUndo,
    #[error("no command with id {0}")]
    UnknownCommand(Uuid),
    #[error("you can only undo your own changes")]
    NotYourCommand,
    #[error("that has already been undone")]
    AlreadyUndone,
    #[error("can't undo: {0}")]
    CannotUndo(String),
}

/// Source of d20 rolls. The server rolls real dice; clients predicting an
/// outcome use [`NoDice`], so anything needing a roll waits for the server.
pub trait Dice {
    /// A roll of a die with `sides` sides.
    fn roll(&mut self, sides: u16) -> Option<u16>;

    /// A d20 roll.
    fn d20(&mut self) -> Option<u8> {
        self.roll(20).and_then(|r| u8::try_from(r).ok())
    }
}

/// Dice that never roll: for predicting, where rolls wait for the server.
pub struct NoDice;

impl Dice for NoDice {
    fn roll(&mut self, _sides: u16) -> Option<u16> {
        None
    }
}

/// Decides what events a command produces. Never returns an empty list: a
/// command that would change nothing is an error, so every accepted command
/// is visible in the log.
pub fn decide(
    state: &CombatState,
    command: &CombatCommand,
    dice: &mut dyn Dice,
) -> Result<Vec<CombatEvent>, CombatError> {
    if let CombatCommand::Undo { .. } | CombatCommand::AddMonsters { .. } = command {
        return Err(CombatError::ServerOnly);
    }
    if state.status == CombatStatus::Ended {
        return Err(CombatError::Ended);
    }
    if state.status == CombatStatus::Paused
        && !matches!(command, CombatCommand::Resume | CombatCommand::End)
    {
        return Err(CombatError::Paused);
    }
    let mut out = Emitter {
        state: state.clone(),
        events: Vec::new(),
    };
    match command {
        CombatCommand::AddCombatant { combatant } => out.add_combatant(combatant)?,
        CombatCommand::RemoveCombatant { combatant } => out.remove(*combatant)?,
        CombatCommand::RollInitiative { combatant, roll } => {
            let roll = match roll {
                Some(r) if (1..=20).contains(r) => *r,
                Some(r) => return Err(CombatError::InvalidRoll(*r)),
                None => dice.d20().ok_or(CombatError::DiceUnavailable)?,
            };
            out.roll_initiative(*combatant, roll)?;
        }
        CombatCommand::RollInitiativeForAll => out.roll_for_all(dice)?,
        CombatCommand::SetInitiative { combatant, total } => {
            out.combatant(*combatant)?;
            out.emit(CombatEvent::InitiativeSet {
                combatant: *combatant,
                total: *total,
            });
        }
        CombatCommand::Start => out.start()?,
        CombatCommand::NextTurn => {
            out.ensure_active()?;
            let (next, wrapped) = out
                .next_after(state.current_turn)
                .ok_or(CombatError::NoInitiative)?;
            out.start_turn(next, wrapped);
        }
        CombatCommand::Damage { combatant, amount } => out.damage(*combatant, *amount)?,
        CombatCommand::Heal { combatant, amount } => out.heal(*combatant, *amount)?,
        CombatCommand::SetTemporaryHitPoints { combatant, amount } => {
            out.set_temporary(*combatant, *amount)?;
        }
        CombatCommand::AddEffect { effect } => out.add_effect(effect)?,
        CombatCommand::RemoveEffect { effect } => {
            state
                .effect(*effect)
                .ok_or(CombatError::UnknownEffect(*effect))?;
            out.emit(CombatEvent::EffectEnded {
                effect: *effect,
                reason: EffectEndReason::Removed,
            });
        }
        CombatCommand::CastSpell {
            caster,
            spell,
            slot,
            targets,
            concentration,
        } => out.cast_spell(*caster, spell, *slot, targets, *concentration)?,
        CombatCommand::UseFeature {
            combatant,
            feature,
            amount,
        } => out.use_feature(*combatant, feature, *amount)?,
        CombatCommand::Pause => {
            out.ensure_active()?;
            out.emit(CombatEvent::Paused);
        }
        CombatCommand::Resume => {
            if state.status != CombatStatus::Paused {
                return Err(CombatError::NotPaused);
            }
            out.emit(CombatEvent::Resumed);
        }
        CombatCommand::End => out.emit(CombatEvent::Ended),
        CombatCommand::Undo { .. } | CombatCommand::AddMonsters { .. } => {
            unreachable!("handled above")
        }
    }
    Ok(out.events)
}

/// What each command does, when it takes more than a line or two. Each
/// starts from the state the command was decided against.
impl Emitter {
    fn add_combatant(&mut self, combatant: &Combatant) -> Result<(), CombatError> {
        if combatant.name.trim().is_empty() {
            return Err(CombatError::EmptyName);
        }
        if combatant.hit_points.current > combatant.hit_points.max {
            return Err(CombatError::InvalidHitPoints);
        }
        self.ensure_unused(combatant.id)?;
        self.emit(CombatEvent::CombatantAdded {
            combatant: combatant.clone(),
        });
        Ok(())
    }

    fn roll_for_all(&mut self, dice: &mut dyn Dice) -> Result<(), CombatError> {
        let unrolled: Vec<_> = self
            .state
            .combatants
            .iter()
            .filter(|c| c.initiative.is_none())
            .map(|c| c.id)
            .collect();
        if unrolled.is_empty() {
            return Err(CombatError::NothingToRoll);
        }
        for id in unrolled {
            let roll = dice.d20().ok_or(CombatError::DiceUnavailable)?;
            self.roll_initiative(id, roll)?;
        }
        Ok(())
    }

    fn start(&mut self) -> Result<(), CombatError> {
        if self.state.status != CombatStatus::Preparing {
            return Err(CombatError::AlreadyStarted);
        }
        let first = self
            .state
            .turn_order()
            .next()
            .ok_or(CombatError::NoInitiative)?
            .id;
        self.emit(CombatEvent::Started);
        self.start_turn(first, true);
        Ok(())
    }

    /// Temporary hit points take damage first. Dropping to 0 breaks
    /// concentration.
    fn damage(&mut self, combatant: Uuid, amount: u32) -> Result<(), CombatError> {
        let before = self.combatant(combatant)?.hit_points;
        let mut hp = before;
        let absorbed = hp.temporary.min(clamp_u16(amount));
        hp.temporary -= absorbed;
        hp.current = hp.current.saturating_sub(clamp_u16(amount) - absorbed);
        self.emit(CombatEvent::Damaged {
            combatant,
            amount,
            hit_points: hp,
        });
        if before.current > 0 && hp.current == 0 {
            self.end_concentration(combatant, EffectEndReason::ConcentrationBroken);
        }
        Ok(())
    }

    fn heal(&mut self, combatant: Uuid, amount: u32) -> Result<(), CombatError> {
        let mut hp = self.combatant(combatant)?.hit_points;
        hp.current = hp.current.saturating_add(clamp_u16(amount)).min(hp.max);
        self.emit(CombatEvent::Healed {
            combatant,
            amount,
            hit_points: hp,
        });
        Ok(())
    }

    fn set_temporary(&mut self, combatant: Uuid, amount: u16) -> Result<(), CombatError> {
        let mut hp = self.combatant(combatant)?.hit_points;
        hp.temporary = amount;
        self.emit(CombatEvent::TemporaryHitPointsSet {
            combatant,
            hit_points: hp,
        });
        Ok(())
    }

    /// A new concentration effect ends its source's old one. An effect
    /// lasting some rounds expires at the start of its source's turn (or,
    /// without a source, of whoever's turn it is now).
    fn add_effect(&mut self, effect: &NewEffect) -> Result<(), CombatError> {
        if effect.name.trim().is_empty() {
            return Err(CombatError::EmptyName);
        }
        self.ensure_unused(effect.id)?;
        for id in effect.source.iter().chain(&effect.targets) {
            self.combatant(*id)?;
        }
        let expires = effect.rounds.map(|rounds| Expiry {
            round: self.state.round + rounds,
            at_turn_of: effect.source.or(self.state.current_turn),
        });
        if let (true, Some(source)) = (effect.concentration, effect.source) {
            self.end_concentration(source, EffectEndReason::ConcentrationReplaced);
        }
        self.emit(CombatEvent::EffectAdded {
            effect: Effect {
                id: effect.id,
                name: effect.name.clone(),
                source: effect.source,
                targets: effect.targets.clone(),
                concentration: effect.concentration,
                expires,
            },
        });
        Ok(())
    }

    /// A cast with `concentration` also puts down the spell as an effect
    /// (with that id), ending the caster's old one.
    fn cast_spell(
        &mut self,
        caster: Uuid,
        spell: &str,
        slot: Option<SpellSlot>,
        targets: &[Uuid],
        concentration: Option<Uuid>,
    ) -> Result<(), CombatError> {
        if spell.trim().is_empty() {
            return Err(CombatError::EmptyName);
        }
        for id in std::iter::once(&caster).chain(targets) {
            self.combatant(*id)?;
        }
        self.emit(CombatEvent::SpellCast {
            caster,
            spell: spell.to_string(),
            slot,
            targets: targets.to_vec(),
        });
        if let Some(effect) = concentration {
            self.ensure_unused(effect)?;
            self.end_concentration(caster, EffectEndReason::ConcentrationReplaced);
            self.emit(CombatEvent::EffectAdded {
                effect: Effect {
                    id: effect,
                    name: spell.to_string(),
                    source: Some(caster),
                    targets: targets.to_vec(),
                    concentration: true,
                    expires: None,
                },
            });
        }
        Ok(())
    }

    fn use_feature(
        &mut self,
        combatant: Uuid,
        feature: &str,
        amount: u16,
    ) -> Result<(), CombatError> {
        if feature.trim().is_empty() {
            return Err(CombatError::EmptyName);
        }
        self.combatant(combatant)?;
        self.emit(CombatEvent::FeatureUsed {
            combatant,
            feature: feature.to_string(),
            amount,
        });
        Ok(())
    }
}

fn clamp_u16(n: u32) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// Collects events while applying them to a scratch copy of the state, so
/// later steps of a command see the effects of earlier ones.
pub(super) struct Emitter {
    pub(super) state: CombatState,
    pub(super) events: Vec<CombatEvent>,
}

impl Emitter {
    pub(super) fn emit(&mut self, event: CombatEvent) {
        self.state.apply(&event);
        self.events.push(event);
    }

    fn combatant(&self, id: Uuid) -> Result<&Combatant, CombatError> {
        self.state
            .combatant(id)
            .ok_or(CombatError::UnknownCombatant(id))
    }

    fn ensure_unused(&self, id: Uuid) -> Result<(), CombatError> {
        if self.state.combatant(id).is_some() || self.state.effect(id).is_some() {
            return Err(CombatError::DuplicateId(id));
        }
        Ok(())
    }

    fn ensure_active(&self) -> Result<(), CombatError> {
        match self.state.status {
            CombatStatus::Active => Ok(()),
            CombatStatus::Preparing => Err(CombatError::NotStarted),
            CombatStatus::Paused => Err(CombatError::Paused),
            CombatStatus::Ended => Err(CombatError::Ended),
        }
    }

    fn roll_initiative(&mut self, id: Uuid, roll: u8) -> Result<(), CombatError> {
        let modifier = self.combatant(id)?.initiative_modifier;
        self.emit(CombatEvent::InitiativeRolled {
            combatant: id,
            roll,
            modifier,
            total: i16::from(roll) + i16::from(modifier),
        });
        Ok(())
    }

    /// The combatant whose turn follows `current`'s, and whether getting
    /// there wraps round to a new round.
    fn next_after(&self, current: Option<Uuid>) -> Option<(Uuid, bool)> {
        let order: Vec<_> = self.state.turn_order().map(|c| c.id).collect();
        let position = current.and_then(|id| order.iter().position(|&c| c == id));
        match position {
            Some(i) if i + 1 < order.len() => Some((order[i + 1], false)),
            _ => order.first().map(|&first| (first, true)),
        }
    }

    /// Starts `next`'s turn, beginning a new round first if `new_round`,
    /// and ends whatever effects expire at that point.
    fn start_turn(&mut self, next: Uuid, new_round: bool) {
        if new_round {
            let round = self.state.round + 1;
            self.emit(CombatEvent::RoundStarted { round });
            // Effects whose source is gone expire at the round boundary.
            self.expire(|state, expiry| {
                expiry
                    .at_turn_of
                    .is_none_or(|id| state.combatant(id).is_none())
            });
        }
        self.emit(CombatEvent::TurnStarted { combatant: next });
        self.expire(|_, expiry| expiry.at_turn_of == Some(next));
    }

    fn expire(&mut self, matches: impl Fn(&CombatState, &Expiry) -> bool) {
        let round = self.state.round;
        let expired: Vec<_> = self
            .state
            .effects
            .iter()
            .filter(|e| {
                e.expires
                    .is_some_and(|x| x.round <= round && matches(&self.state, &x))
            })
            .map(|e| e.id)
            .collect();
        for effect in expired {
            self.emit(CombatEvent::EffectEnded {
                effect,
                reason: EffectEndReason::Expired,
            });
        }
    }

    fn end_concentration(&mut self, source: Uuid, reason: EffectEndReason) {
        let concentrating: Vec<_> = self
            .state
            .effects
            .iter()
            .filter(|e| e.concentration && e.source == Some(source))
            .map(|e| e.id)
            .collect();
        for effect in concentrating {
            self.emit(CombatEvent::EffectEnded { effect, reason });
        }
    }

    pub(super) fn remove(&mut self, id: Uuid) -> Result<(), CombatError> {
        self.combatant(id)?;
        let was_current = self.state.current_turn == Some(id);
        // Work out who's next while the leaver is still in the order.
        let next = self.next_after(Some(id)).filter(|&(n, _)| n != id);
        self.end_concentration(id, EffectEndReason::SourceRemoved);
        self.emit(CombatEvent::CombatantRemoved { combatant: id });
        if was_current
            && self.state.status == CombatStatus::Active
            && let Some((next, wrapped)) = next
        {
            self.start_turn(next, wrapped);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dice that roll a fixed sequence.
    struct Loaded(Vec<u8>);

    impl Dice for Loaded {
        fn roll(&mut self, _sides: u16) -> Option<u16> {
            Some(u16::from(self.0.remove(0)))
        }
    }

    struct Fixture {
        state: CombatState,
    }

    impl Fixture {
        fn new() -> Self {
            let mut state = CombatState::default();
            state.apply(&CombatEvent::Created {
                name: "Ambush".into(),
                campaign: "Test".into(),
            });
            Self { state }
        }

        fn add(&mut self, name: &str, modifier: i8, hp: u16) -> Uuid {
            let mut c = Combatant::new(name, hp);
            c.initiative_modifier = modifier;
            let id = c.id;
            self.ok(CombatCommand::AddCombatant { combatant: c });
            id
        }

        #[expect(
            clippy::needless_pass_by_value,
            reason = "tests read better passing commands by value"
        )]
        fn run(&mut self, command: CombatCommand) -> Result<Vec<CombatEvent>, CombatError> {
            let events = decide(&self.state, &command, &mut NoDice)?;
            for e in &events {
                self.state.apply(e);
            }
            Ok(events)
        }

        fn ok(&mut self, command: CombatCommand) -> Vec<CombatEvent> {
            self.run(command).unwrap()
        }

        fn init(&mut self, id: Uuid, total: i16) {
            self.ok(CombatCommand::SetInitiative {
                combatant: id,
                total,
            });
        }

        fn names(&self) -> Vec<&str> {
            self.state.turn_order().map(|c| c.name.as_str()).collect()
        }

        fn current(&self) -> &str {
            let id = self.state.current_turn.unwrap();
            &self.state.combatant(id).unwrap().name
        }

        fn effect(
            &mut self,
            name: &str,
            source: Uuid,
            concentration: bool,
            rounds: Option<u32>,
        ) -> Uuid {
            let id = Uuid::new_v4();
            self.ok(CombatCommand::AddEffect {
                effect: NewEffect {
                    id,
                    name: name.into(),
                    source: Some(source),
                    targets: vec![],
                    concentration,
                    rounds,
                },
            });
            id
        }
    }

    #[test]
    fn initiative_order_breaks_ties_by_modifier_then_join_order() {
        let mut f = Fixture::new();
        let grog = f.add("Grog", 0, 50);
        let vex = f.add("Vex", 4, 40);
        let pike = f.add("Pike", 0, 30);
        let goblin = f.add("Goblin", 2, 7);
        f.init(grog, 15);
        f.init(vex, 15);
        f.init(pike, 15);
        assert_eq!(f.names(), ["Vex", "Grog", "Pike"], "goblin not rolled yet");
        f.init(goblin, 20);
        assert_eq!(f.names(), ["Goblin", "Vex", "Grog", "Pike"]);
    }

    #[test]
    fn server_rolls_use_dice_and_clients_cannot_predict_them() {
        let mut f = Fixture::new();
        let a = f.add("A", 3, 10);
        let b = f.add("B", -1, 10);
        let events = decide(
            &f.state,
            &CombatCommand::RollInitiativeForAll,
            &mut Loaded(vec![12, 18]),
        )
        .unwrap();
        assert_eq!(
            events,
            [
                CombatEvent::InitiativeRolled {
                    combatant: a,
                    roll: 12,
                    modifier: 3,
                    total: 15
                },
                CombatEvent::InitiativeRolled {
                    combatant: b,
                    roll: 18,
                    modifier: -1,
                    total: 17
                },
            ]
        );
        assert_eq!(
            f.run(CombatCommand::RollInitiativeForAll),
            Err(CombatError::DiceUnavailable)
        );
        // A physical roll the player supplies is predictable.
        f.ok(CombatCommand::RollInitiative {
            combatant: a,
            roll: Some(20),
        });
        assert_eq!(f.state.combatant(a).unwrap().initiative, Some(23));
        assert_eq!(
            f.run(CombatCommand::RollInitiative {
                combatant: a,
                roll: Some(21)
            }),
            Err(CombatError::InvalidRoll(21))
        );
    }

    #[test]
    fn turns_advance_and_wrap_into_new_rounds() {
        let mut f = Fixture::new();
        let a = f.add("A", 0, 10);
        let b = f.add("B", 0, 10);
        assert_eq!(f.run(CombatCommand::Start), Err(CombatError::NoInitiative));
        f.init(a, 10);
        f.init(b, 5);
        assert_eq!(f.run(CombatCommand::NextTurn), Err(CombatError::NotStarted));

        f.ok(CombatCommand::Start);
        assert_eq!((f.state.round, f.current()), (1, "A"));
        f.ok(CombatCommand::NextTurn);
        assert_eq!((f.state.round, f.current()), (1, "B"));
        let events = f.ok(CombatCommand::NextTurn);
        assert_eq!(events[0], CombatEvent::RoundStarted { round: 2 });
        assert_eq!((f.state.round, f.current()), (2, "A"));
        assert_eq!(
            f.run(CombatCommand::Start),
            Err(CombatError::AlreadyStarted)
        );
    }

    #[test]
    fn combatants_can_join_mid_combat() {
        let mut f = Fixture::new();
        let a = f.add("A", 0, 10);
        let b = f.add("B", 0, 10);
        f.init(a, 20);
        f.init(b, 5);
        f.ok(CombatCommand::Start);

        let mut late = Combatant::new("Reinforcement", 10);
        late.initiative = Some(12);
        f.ok(CombatCommand::AddCombatant { combatant: late });
        assert_eq!(f.names(), ["A", "Reinforcement", "B"]);
        f.ok(CombatCommand::NextTurn);
        assert_eq!(f.current(), "Reinforcement");
    }

    #[test]
    fn removing_the_current_combatant_passes_the_turn() {
        let mut f = Fixture::new();
        let a = f.add("A", 0, 10);
        let b = f.add("B", 0, 10);
        let c = f.add("C", 0, 10);
        f.init(a, 3);
        f.init(b, 2);
        f.init(c, 1);
        f.ok(CombatCommand::Start);
        f.ok(CombatCommand::NextTurn);
        f.ok(CombatCommand::NextTurn);
        assert_eq!(f.current(), "C");

        // C is last, so the turn wraps to A in a new round.
        f.ok(CombatCommand::RemoveCombatant { combatant: c });
        assert_eq!((f.state.round, f.current()), (2, "A"));
        // A is first in the order; removing them must not start a round.
        f.ok(CombatCommand::RemoveCombatant { combatant: a });
        assert_eq!((f.state.round, f.current()), (2, "B"));
        f.ok(CombatCommand::RemoveCombatant { combatant: b });
        assert_eq!(f.state.current_turn, None);
    }

    #[test]
    fn damage_hits_temporary_hp_first_and_heal_caps_at_max() {
        let mut f = Fixture::new();
        let grog = f.add("Grog", 0, 50);
        f.ok(CombatCommand::SetTemporaryHitPoints {
            combatant: grog,
            amount: 5,
        });
        f.ok(CombatCommand::Damage {
            combatant: grog,
            amount: 8,
        });
        let hp = f.state.combatant(grog).unwrap().hit_points;
        assert_eq!((hp.current, hp.temporary), (47, 0));
        f.ok(CombatCommand::Heal {
            combatant: grog,
            amount: 100,
        });
        assert_eq!(f.state.combatant(grog).unwrap().hit_points.current, 50);
        f.ok(CombatCommand::Damage {
            combatant: grog,
            amount: u32::MAX,
        });
        assert!(f.state.combatant(grog).unwrap().is_down());
    }

    #[test]
    fn concentration_is_replaced_and_broken() {
        let mut f = Fixture::new();
        let keyleth = f.add("Keyleth", 0, 30);
        let moonbeam = f.effect("Moonbeam", keyleth, true, Some(10));
        let barkskin = f.effect("Barkskin", keyleth, false, None);

        let events = f.ok(CombatCommand::AddEffect {
            effect: NewEffect {
                id: Uuid::new_v4(),
                name: "Faerie Fire".into(),
                source: Some(keyleth),
                targets: vec![],
                concentration: true,
                rounds: Some(10),
            },
        });
        assert_eq!(
            events[0],
            CombatEvent::EffectEnded {
                effect: moonbeam,
                reason: EffectEndReason::ConcentrationReplaced
            }
        );

        f.ok(CombatCommand::Damage {
            combatant: keyleth,
            amount: 30,
        });
        let names: Vec<_> = f.state.effects.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Barkskin"], "only the concentration effect ends");
        assert!(f.state.effect(barkskin).is_some());
    }

    #[test]
    fn effects_expire_at_the_start_of_the_sources_turn() {
        let mut f = Fixture::new();
        let cleric = f.add("Cleric", 0, 10);
        let orc = f.add("Orc", 0, 10);
        f.init(cleric, 10);
        f.init(orc, 5);
        f.ok(CombatCommand::Start);
        // Round 1, cleric's turn: Bless for 2 rounds ends at the start of
        // the cleric's turn in round 3.
        let bless = f.effect("Bless", cleric, true, Some(2));
        f.ok(CombatCommand::NextTurn); // R1 orc
        f.ok(CombatCommand::NextTurn); // R2 cleric
        f.ok(CombatCommand::NextTurn); // R2 orc
        assert!(f.state.effect(bless).is_some());
        let events = f.ok(CombatCommand::NextTurn); // R3 cleric
        assert!(events.contains(&CombatEvent::EffectEnded {
            effect: bless,
            reason: EffectEndReason::Expired
        }));
        assert!(f.state.effects.is_empty());
    }

    #[test]
    fn effects_without_a_source_or_targets_count_the_active_combatants_turns() {
        // Regression: an effect nobody cast ended at the start of a round,
        // whoever's turn it was when it was put down.
        let mut f = Fixture::new();
        let a = f.add("A", 0, 10);
        let b = f.add("B", 0, 10);
        f.init(a, 10);
        f.init(b, 5);
        f.ok(CombatCommand::Start);
        f.ok(CombatCommand::NextTurn); // round 1, B's turn
        let fire = Uuid::new_v4();
        f.ok(CombatCommand::AddEffect {
            effect: NewEffect {
                id: fire,
                name: "Fire".into(),
                source: None,
                targets: vec![],
                concentration: false,
                rounds: Some(1),
            },
        });
        assert_eq!(
            f.state.effect(fire).unwrap().expires.unwrap().at_turn_of,
            Some(b)
        );
        f.ok(CombatCommand::NextTurn); // round 2, A's turn
        assert!(
            f.state.effect(fire).is_some(),
            "lasts until B's turn comes round"
        );
        let events = f.ok(CombatCommand::NextTurn); // round 2, B's turn
        assert!(events.contains(&CombatEvent::EffectEnded {
            effect: fire,
            reason: EffectEndReason::Expired
        }));
    }

    #[test]
    fn effects_of_departed_sources_expire_at_round_start() {
        let mut f = Fixture::new();
        let a = f.add("A", 0, 10);
        let b = f.add("B", 0, 10);
        f.init(a, 10);
        f.init(b, 5);
        f.ok(CombatCommand::Start);
        let web = f.effect("Web", b, false, Some(1));
        f.ok(CombatCommand::RemoveCombatant { combatant: b });
        assert!(
            f.state.effect(web).is_some(),
            "not concentration, so it lingers"
        );
        f.ok(CombatCommand::NextTurn); // round 2 starts
        assert!(f.state.effect(web).is_none());
    }

    #[test]
    fn ended_combat_rejects_everything_and_ids_are_unique() {
        let mut f = Fixture::new();
        let a = f.add("A", 0, 10);
        let mut dup = Combatant::new("Copy", 5);
        dup.id = a;
        assert_eq!(
            f.run(CombatCommand::AddCombatant { combatant: dup }),
            Err(CombatError::DuplicateId(a))
        );
        f.ok(CombatCommand::End);
        assert_eq!(
            f.run(CombatCommand::Heal {
                combatant: a,
                amount: 1
            }),
            Err(CombatError::Ended)
        );
    }

    #[test]
    fn spells_and_features_are_recorded_and_concentration_comes_into_play() {
        let mut f = Fixture::new();
        let keyleth = f.add("Keyleth", 0, 30);
        let goblin = f.add("Goblin", 0, 7);
        let moonbeam = f.effect("Moonbeam", keyleth, true, None);
        let bless = Uuid::new_v4();
        let events = f.ok(CombatCommand::CastSpell {
            caster: keyleth,
            spell: "Bless".into(),
            slot: Some(SpellSlot::Level(1)),
            targets: vec![goblin],
            concentration: Some(bless),
        });
        assert_eq!(
            events[0],
            CombatEvent::SpellCast {
                caster: keyleth,
                spell: "Bless".into(),
                slot: Some(SpellSlot::Level(1)),
                targets: vec![goblin],
            }
        );
        assert!(f.state.effect(moonbeam).is_none(), "concentration moved on");
        let effect = f.state.effect(bless).unwrap();
        assert_eq!((effect.source, effect.concentration), (Some(keyleth), true));

        let events = f.ok(CombatCommand::CastSpell {
            caster: keyleth,
            spell: "Fire Bolt".into(),
            slot: None,
            targets: vec![goblin],
            concentration: None,
        });
        assert_eq!(events.len(), 1, "nothing comes into play");
        assert!(f.state.effect(bless).is_some(), "still concentrating");
        assert_eq!(
            f.run(CombatCommand::CastSpell {
                caster: keyleth,
                spell: "Fire Bolt".into(),
                slot: None,
                targets: vec![Uuid::nil()],
                concentration: None,
            }),
            Err(CombatError::UnknownCombatant(Uuid::nil()))
        );

        let used = f.ok(CombatCommand::UseFeature {
            combatant: keyleth,
            feature: "Wild Shape".into(),
            amount: 1,
        });
        assert!(
            matches!(&used[0], CombatEvent::FeatureUsed { feature, .. } if feature == "Wild Shape")
        );
    }

    #[test]
    fn paused_combat_only_resumes_or_ends() {
        let mut f = Fixture::new();
        let a = f.add("A", 0, 10);
        f.init(a, 10);
        f.ok(CombatCommand::Start);
        f.ok(CombatCommand::Pause);
        assert_eq!(f.state.status, CombatStatus::Paused);
        assert_eq!(f.run(CombatCommand::NextTurn), Err(CombatError::Paused));
        assert_eq!(f.run(CombatCommand::Pause), Err(CombatError::Paused));
        f.ok(CombatCommand::Resume);
        assert_eq!(f.state.status, CombatStatus::Active);
        assert_eq!(f.state.round, 1, "picks up where it left off");
        assert_eq!(f.run(CombatCommand::Resume), Err(CombatError::NotPaused));
        f.ok(CombatCommand::Pause);
        f.ok(CombatCommand::End);
    }

    #[test]
    fn replaying_the_log_rebuilds_the_state() {
        let mut f = Fixture::new();
        let mut log = vec![CombatEvent::Created {
            name: "Ambush".into(),
            campaign: "Test".into(),
        }];
        let a = f.add("A", 0, 10);
        log.extend(
            decide(
                &CombatState::replay(&log),
                &CombatCommand::AddCombatant {
                    combatant: f.state.combatant(a).unwrap().clone(),
                },
                &mut NoDice,
            )
            .unwrap(),
        );
        assert_eq!(CombatState::replay(&log), f.state);
    }

    #[test]
    fn json_shapes() {
        let cmd: CombatCommand = serde_json::from_str(
            r#"{"type":"damage","combatant":"00000000-0000-0000-0000-000000000001","amount":7}"#,
        )
        .unwrap();
        assert!(matches!(cmd, CombatCommand::Damage { amount: 7, .. }));
        let roll: CombatCommand = serde_json::from_str(
            r#"{"type":"roll_initiative","combatant":"00000000-0000-0000-0000-000000000001"}"#,
        )
        .unwrap();
        assert!(matches!(
            roll,
            CombatCommand::RollInitiative { roll: None, .. }
        ));
    }
}
