//! The fight screen's command line: what a line means, in words ("Vex deals
//! 7 damage to Goblin") and as steps to run, and what could come next.
//!
//! A line is `[WHO] ACTION [ARGS]`. Combatants are referred to by their
//! number in the initiative order. `WHO` is optional and defaults to whoever
//! has the turn, so `damage 3 7` is the current combatant hitting number 3,
//! and `2 cast bless 1 4` is number 2 casting Bless on 1 and 4. Everything
//! but self-targeted actions (`use`) takes any combatants as targets.

mod interpret;
mod names;
mod suggest;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use edda_types::{
    Character, CharacterSheet, CharacterSummary, Spell,
    combat::{CombatCommand, CombatError, CombatState, Combatant, NoDice, decide, player_may},
    monster::MonsterSummary,
    rules_text::{self, ConditionEntry, SpellEntry},
};
use uuid::Uuid;

use interpret::{interpret, section, topic, verb};

use crate::display::Section;
pub use suggest::suggest;

/// What a line is interpreted against.
pub struct Context<'a> {
    pub state: &'a CombatState,
    /// Sheets of the combatants who are characters, by character id.
    pub sheets: &'a HashMap<Uuid, Character>,
    /// The campaign's characters, who can be added.
    pub roster: &'a [CharacterSummary],
    /// The monsters that can be added.
    pub bestiary: &'a [MonsterSummary],
    /// The spells' and conditions' descriptions.
    pub spells: &'a [SpellEntry],
    pub conditions: &'a [ConditionEntry],
    /// The player device using the screen, by key; `None` for a DM device.
    /// A player changes only their own combatants.
    pub player: Option<&'a str>,
}

/// What a line is about, whose description the side panel can show.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Topic {
    /// A spell, by name (perhaps only partly typed).
    Spell(String),
    /// An effect in play or a condition, by name (perhaps partly typed).
    Effect(String),
    /// Anything with a description: an effect in play, a spell or a
    /// condition, by name.
    Named(String),
    Combatant(Uuid),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A combat command. What it costs a sheet (a slot, uses) the combat
    /// service spends.
    Combat(CombatCommand),
    /// Add a campaign character, as their sheet currently stands.
    Join(Uuid),
    /// Keep a description in the side panel.
    Show(Topic),
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// Who acts: the line's `WHO`, or whoever has the turn.
    pub actor: Option<Uuid>,
    /// The line in words, with `?` for what's still missing.
    pub sentence: String,
    /// Ready to run, or what's missing or wrong.
    pub plan: Result<Vec<Step>, String>,
    /// The part of the actor's sheet that the line needs, such as
    /// spellcasting for a cast.
    pub section: Option<Section>,
    /// What the line is about, to describe.
    pub topic: Option<Topic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Damage,
    Heal,
    Temp,
    Cast,
    Effect,
    Drop,
    Show,
    Use,
    Init,
    Remove,
    Add,
    Roll,
    Start,
    Next,
    Pause,
    Resume,
    End,
    Undo,
    Quit,
}

/// Every action with its arguments, in the order they're offered.
/// How many actions there are: what an empty line lists.
pub const ACTIONS: usize = VERBS.len();

const VERBS: &[(Verb, &str, &str, &str)] = &[
    (Verb::Damage, "damage", "<who…> <amount>", "deal damage"),
    (Verb::Heal, "heal", "<who…> <amount>", "restore hit points"),
    (
        Verb::Temp,
        "temp",
        "<who…> <amount>",
        "give temporary hit points",
    ),
    (
        Verb::Cast,
        "cast",
        "<spell> [who…] [at <level>]",
        "cast a spell",
    ),
    (
        Verb::Effect,
        "effect",
        "<name> [who…] [for <rounds>]",
        "put an effect on",
    ),
    (Verb::Drop, "drop", "<effect>", "end an effect"),
    (
        Verb::Show,
        "show",
        "<spell | effect | condition | who>",
        "describe it in the side panel",
    ),
    (
        Verb::Use,
        "use",
        "<feature> [amount]",
        "use one's own feature",
    ),
    (Verb::Next, "next", "", "end the turn"),
    (
        Verb::Init,
        "init",
        "<who> [total]",
        "set or roll initiative",
    ),
    (
        Verb::Roll,
        "roll",
        "",
        "roll initiative for everyone without",
    ),
    (
        Verb::Add,
        "add",
        "<character> | [count] <monster> [roll] | <name> <hp>",
        "join the combat",
    ),
    (Verb::Remove, "remove", "<who…>", "leave the combat"),
    (Verb::Start, "start", "", "begin the first round"),
    (Verb::Undo, "undo", "", "take back your last change"),
    (Verb::Pause, "pause", "", "pause the combat"),
    (Verb::Resume, "resume", "", "resume a paused combat"),
    (Verb::End, "end", "", "end the combat"),
    (Verb::Quit, "quit", "", "leave this screen"),
];

impl Context<'_> {
    /// The combatant numbered `n` (from 1) in initiative order.
    fn numbered(&self, n: usize) -> Option<&Combatant> {
        n.checked_sub(1).and_then(|i| self.state.combatants.get(i))
    }

    fn by_id(&self, id: Uuid) -> Option<&Combatant> {
        self.state.combatant(id)
    }

    fn sheet(&self, combatant: &Combatant) -> Option<&CharacterSheet> {
        let id = combatant.character_id?;
        self.sheets.get(&id).map(|c| &c.sheet)
    }

    /// The combatant a target word refers to.
    fn target(&self, word: &str) -> Result<&Combatant, String> {
        match word.parse::<usize>() {
            Ok(n) => self
                .numbered(n)
                .ok_or_else(|| format!("there's no number {n}")),
            Err(_) => Err(format!("press Tab to pick \"{word}\"")),
        }
    }
}

/// What `line` means.
pub fn parse(line: &str, ctx: &Context) -> Parsed {
    let words: Vec<&str> = line.split_whitespace().collect();
    let turn = ctx.state.current_turn;
    let mut parsed = Parsed {
        actor: turn,
        sentence: String::new(),
        plan: Err("type an action; Tab completes".into()),
        section: None,
        topic: None,
    };
    let (who, rest) = match words.split_first() {
        None => return parsed,
        Some((first, rest)) => match first.parse::<usize>() {
            Ok(n) => {
                if let Some(c) = ctx.numbered(n) {
                    (Some(c), rest)
                } else {
                    parsed.plan = Err(format!("there's no number {n}"));
                    return parsed;
                }
            }
            Err(_) => (None, &words[..]),
        },
    };
    if let Some(who) = who {
        parsed.actor = Some(who.id);
    }
    let actor = parsed.actor.and_then(|id| ctx.by_id(id));
    let Some((&verb_word, args)) = rest.split_first() else {
        parsed.sentence = actor.map(|a| format!("{} …", a.name)).unwrap_or_default();
        parsed.plan = Err("what do they do? Tab lists the actions".into());
        return parsed;
    };
    let Some(verb) = verb(verb_word) else {
        parsed.plan = Err(format!("\"{verb_word}\" isn't an action; Tab lists them"));
        return parsed;
    };
    parsed.section = section(verb, actor);
    parsed.topic = topic(verb, actor, args, ctx);
    let (sentence, plan) = interpret(verb, actor, args, ctx);
    parsed.sentence = sentence;
    parsed.plan = plan.and_then(|steps| check(steps, ctx));
    parsed
}

impl Context<'_> {
    /// Whether there's a description of what `topic` names, exactly.
    pub fn describes(&self, topic: &Topic) -> bool {
        let spell = |name: &str| rules_text::named(self.spells, name).is_some();
        let condition = |name: &str| rules_text::named(self.conditions, name).is_some();
        let effect = |name: &str| {
            self.state
                .effects
                .iter()
                .any(|e| e.name.eq_ignore_ascii_case(name.trim()))
        };
        match topic {
            Topic::Spell(name) => spell(name) || self.known_spell(name).is_some(),
            Topic::Effect(name) => effect(name) || spell(name) || condition(name),
            Topic::Named(name) => {
                effect(name)
                    || spell(name)
                    || condition(name)
                    || self.named_combatant(name).is_some()
            }
            Topic::Combatant(id) => self.by_id(*id).is_some(),
        }
    }

    /// A spell called `name` on any fighting character's sheet.
    pub fn known_spell(&self, name: &str) -> Option<&Spell> {
        self.state
            .combatants
            .iter()
            .filter_map(|c| self.sheet(c))
            .find_map(|sheet| sheet.spellcasting.find_spell(name.trim()))
            .filter(|s| s.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Whether `combatant` plays a character the device `device` owns.
    fn played_by(&self, device: &str, combatant: &Combatant) -> bool {
        self.roster.iter().any(|character| {
            Some(character.id) == combatant.character_id
                && character.owners.iter().any(|owner| owner == device)
        })
    }

    /// The combatant called `name`, in any case.
    pub fn named_combatant(&self, name: &str) -> Option<&Combatant> {
        self.state
            .combatants
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name.trim()))
    }
}

/// Catches combat commands that would certainly be refused, before sending:
/// by the rules, or because a player may not.
fn check(steps: Vec<Step>, ctx: &Context) -> Result<Vec<Step>, String> {
    let mut state = ctx.state.clone();
    for step in &steps {
        if let (Some(_), Step::Join(_)) = (ctx.player, step) {
            return Err(CombatError::DmOnly.to_string());
        }
        if let Step::Combat(command) = step {
            if let Some(device) = ctx.player {
                player_may(&state, command, |c| ctx.played_by(device, c))
                    .map_err(|e| e.to_string())?;
            }
            match decide(&state, command, &mut NoDice) {
                Ok(events) => events.iter().for_each(|e| state.apply(e)),
                Err(CombatError::DiceUnavailable | CombatError::ServerOnly) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    Ok(steps)
}
