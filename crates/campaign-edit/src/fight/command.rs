//! The fight screen's command line: what a line means, in words ("Vex deals
//! 7 damage to Goblin") and as steps to run, and what could come next.
//!
//! A line is `[WHO] ACTION [ARGS]`. Combatants are referred to by their
//! number in the initiative order. `WHO` is optional and defaults to whoever
//! has the turn, so `damage 3 7` is the current combatant hitting number 3,
//! and `2 cast bless 1 4` is number 2 casting Bless on 1 and 4. Everything
//! but self-targeted actions (`use`) takes any combatants as targets.

use std::collections::HashMap;

use campaign_types::{
    Character, CharacterAction, CharacterSheet, CharacterSummary, Spell,
    combat::{
        CombatCommand, CombatError, CombatState, Combatant, NewEffect, NoDice, SpellSlot, decide,
    },
    monster::MonsterSummary,
};
use uuid::Uuid;

/// What a line is interpreted against.
pub struct Context<'a> {
    pub state: &'a CombatState,
    /// Sheets of the combatants who are characters, by character id.
    pub sheets: &'a HashMap<Uuid, Character>,
    /// The campaign's characters, who can be added.
    pub roster: &'a [CharacterSummary],
    /// The monsters that can be added.
    pub bestiary: &'a [MonsterSummary],
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Combat(CombatCommand),
    /// Something done to a character's sheet, like spending a slot.
    Character {
        id: Uuid,
        action: CharacterAction,
    },
    /// Add a campaign character, as their sheet currently stands.
    Join(Uuid),
    /// Add `count` of a monster, at their maximum hit points or rolled.
    Summon {
        monster: Uuid,
        count: u16,
        roll: bool,
    },
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// The whole line once this is accepted.
    pub line: String,
    pub label: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Damage,
    Heal,
    Temp,
    Cast,
    Effect,
    Drop,
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
    };
    let (who, rest) = match words.split_first() {
        None => return parsed,
        Some((first, rest)) => match first.parse::<usize>() {
            Ok(n) => match ctx.numbered(n) {
                Some(c) => (Some(c), rest),
                None => {
                    parsed.plan = Err(format!("there's no number {n}"));
                    return parsed;
                }
            },
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
    let (sentence, plan) = interpret(verb, actor, args, ctx);
    parsed.sentence = sentence;
    parsed.plan = plan.and_then(|steps| check(steps, ctx));
    parsed
}

/// The verb a word names, also by a unique prefix.
fn verb(word: &str) -> Option<Verb> {
    let word = word.to_lowercase();
    if let Some((verb, ..)) = VERBS.iter().find(|(_, name, ..)| *name == word) {
        return Some(*verb);
    }
    let mut matching = VERBS.iter().filter(|(_, name, ..)| name.starts_with(&word));
    match (matching.next(), matching.next()) {
        (Some((verb, ..)), None) => Some(*verb),
        _ => None,
    }
}

type Interpreted = (String, Result<Vec<Step>, String>);

fn interpret(verb: Verb, actor: Option<&Combatant>, args: &[&str], ctx: &Context) -> Interpreted {
    let who = |a: Option<&Combatant>| a.map_or("?".to_string(), |a| a.name.clone());
    let bare = |sentence: String, command: CombatCommand| -> Interpreted {
        if args.is_empty() {
            (sentence, Ok(vec![Step::Combat(command)]))
        } else {
            (sentence, Err("nothing more needed; press Enter".into()))
        }
    };
    match verb {
        Verb::Damage | Verb::Heal | Verb::Temp => amount_verb(verb, actor, args, ctx),
        Verb::Cast => cast(actor, args, ctx),
        Verb::Effect => effect(actor, args, ctx),
        Verb::Drop => {
            let name = args.join(" ");
            let found = ctx
                .state
                .effects
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(&name));
            match found {
                Some(e) => (
                    format!("{} ends", e.name),
                    Ok(vec![Step::Combat(CombatCommand::RemoveEffect {
                        effect: e.id,
                    })]),
                ),
                None if name.is_empty() => ("? ends".into(), Err("which effect?".into())),
                None => (
                    format!("{name} ends"),
                    Err(format!("no effect called \"{name}\" is in play")),
                ),
            }
        }
        Verb::Use => use_feature(actor, args, ctx),
        Verb::Init => {
            let Some((first, rest)) = args.split_first() else {
                return (
                    "?'s initiative".into(),
                    Err("whose? type their number".into()),
                );
            };
            let target = match ctx.target(first) {
                Ok(t) => t,
                Err(e) => return ("?'s initiative".into(), Err(e)),
            };
            match rest {
                [] => (
                    format!("{} rolls initiative", target.name),
                    Ok(vec![Step::Combat(CombatCommand::RollInitiative {
                        combatant: target.id,
                        roll: None,
                    })]),
                ),
                [total] => match total.parse::<i16>() {
                    Ok(total) => (
                        format!("{}'s initiative is {total}", target.name),
                        Ok(vec![Step::Combat(CombatCommand::SetInitiative {
                            combatant: target.id,
                            total,
                        })]),
                    ),
                    Err(_) => (
                        format!("{}'s initiative is ?", target.name),
                        Err(format!("\"{total}\" isn't a number")),
                    ),
                },
                _ => (
                    format!("{}'s initiative", target.name),
                    Err("just a number for the total".into()),
                ),
            }
        }
        Verb::Remove => match targets(args, ctx) {
            Ok(ts) if ts.is_empty() => ("? leaves".into(), Err("who? type their number".into())),
            Ok(ts) => (
                format!(
                    "{} {} the combat",
                    names(&ts),
                    verb_s(&ts, "leaves", "leave")
                ),
                Ok(ts
                    .iter()
                    .map(|t| Step::Combat(CombatCommand::RemoveCombatant { combatant: t.id }))
                    .collect()),
            ),
            Err(e) => ("? leaves".into(), Err(e)),
        },
        Verb::Add => add(args, ctx),
        Verb::Roll => bare(
            "Everyone without initiative rolls".into(),
            CombatCommand::RollInitiativeForAll,
        ),
        Verb::Start => bare("The combat starts".into(), CombatCommand::Start),
        Verb::Next => {
            let current = ctx.state.current_turn.and_then(|id| ctx.by_id(id));
            bare(
                format!("{}'s turn ends", who(current)),
                CombatCommand::NextTurn,
            )
        }
        Verb::Pause => bare("The combat pauses".into(), CombatCommand::Pause),
        Verb::Resume => bare("The combat resumes".into(), CombatCommand::Resume),
        Verb::End => bare("The combat ends".into(), CombatCommand::End),
        Verb::Undo => bare(
            "Take back your last change".into(),
            CombatCommand::Undo { command: None },
        ),
        Verb::Quit => {
            if args.is_empty() {
                ("Leave the fight screen".into(), Ok(vec![Step::Quit]))
            } else {
                (
                    "Leave the fight screen".into(),
                    Err("nothing more needed".into()),
                )
            }
        }
    }
}

/// `damage`, `heal` and `temp`: targets, then the amount.
fn amount_verb(verb: Verb, actor: Option<&Combatant>, args: &[&str], ctx: &Context) -> Interpreted {
    // One number is who; the amount comes last after that.
    let (target_words, amount) = match args {
        [] | [_] => (args, None),
        [targets @ .., last] => (targets, Some(*last)),
    };
    let targets = targets(target_words, ctx);
    let amount_value = amount.map(|a| a.parse::<u32>());
    let shown_targets = match &targets {
        Ok(ts) if !ts.is_empty() => names(ts),
        _ => "?".into(),
    };
    let shown_amount = match &amount_value {
        Some(Ok(n)) => n.to_string(),
        _ => "?".into(),
    };
    let sentence = match (verb, actor) {
        (Verb::Damage, Some(a)) => {
            format!("{} deals {shown_amount} damage to {shown_targets}", a.name)
        }
        (Verb::Damage, None) => format!("{shown_targets} takes {shown_amount} damage"),
        (Verb::Heal, Some(a)) => format!("{} heals {shown_targets} for {shown_amount}", a.name),
        (Verb::Heal, None) => format!("{shown_targets} regains {shown_amount} hit points"),
        (_, Some(a)) => format!(
            "{} gives {shown_targets} {shown_amount} temporary hit points",
            a.name
        ),
        (_, None) => format!("{shown_targets} gains {shown_amount} temporary hit points"),
    };
    let targets = match targets {
        Ok(ts) if ts.is_empty() => return (sentence, Err("who? type their number".into())),
        Ok(ts) => ts,
        Err(e) => return (sentence, Err(e)),
    };
    let amount = match amount_value {
        None => return (sentence, Err("how much? type the amount".into())),
        Some(Err(_)) => {
            return (
                sentence,
                Err(format!(
                    "\"{}\" isn't an amount",
                    amount.unwrap_or_default()
                )),
            );
        }
        Some(Ok(n)) => n,
    };
    let steps = targets
        .iter()
        .map(|t| {
            Step::Combat(match verb {
                Verb::Damage => CombatCommand::Damage {
                    combatant: t.id,
                    amount,
                },
                Verb::Heal => CombatCommand::Heal {
                    combatant: t.id,
                    amount,
                },
                _ => CombatCommand::SetTemporaryHitPoints {
                    combatant: t.id,
                    amount: u16::try_from(amount).unwrap_or(u16::MAX),
                },
            })
        })
        .collect();
    (sentence, Ok(steps))
}

fn cast(actor: Option<&Combatant>, args: &[&str], ctx: &Context) -> Interpreted {
    let Some(actor) = actor else {
        return (
            "? casts …".into(),
            Err("who's casting? start with their number".into()),
        );
    };
    let Some(sheet) = ctx.sheet(actor) else {
        return (
            format!("{} casts …", actor.name),
            Err(format!("{} has no character sheet", actor.name)),
        );
    };
    let spells = &sheet.spellcasting.spells;
    let Some((spell, used)) = longest_name(args, spells.iter().map(|s| s.name.as_str()))
        .and_then(|(name, used)| sheet.spellcasting.find_spell(name).map(|s| (s, used)))
    else {
        let typed = args.join(" ");
        let error = if typed.is_empty() {
            "which spell? Tab lists them".to_string()
        } else {
            format!(
                "{} doesn't know \"{typed}\"; Tab lists their spells",
                actor.name
            )
        };
        return (format!("{} casts …", actor.name), Err(error));
    };

    let mut target_words = Vec::new();
    let mut at_level = None;
    let mut pact = false;
    let mut rest = args[used..].iter();
    let mut problem = None;
    while let Some(word) = rest.next() {
        match word.to_lowercase().as_str() {
            "at" => match rest
                .next()
                .map(|l| l.trim_end_matches(|c: char| c.is_alphabetic()))
            {
                Some(level) => match level.parse::<u8>() {
                    Ok(level) => at_level = Some(level),
                    Err(_) => problem = Some(format!("\"{level}\" isn't a spell level")),
                },
                None => problem = Some("at which level?".into()),
            },
            "pact" => pact = true,
            _ => target_words.push(*word),
        }
    }
    let targets = targets(&target_words, ctx);
    let mut sentence = format!("{} casts {}", actor.name, spell.name);
    if let Some(level) = at_level {
        sentence += &format!(" at {}", crate::display::ordinal(level));
    }
    if let Ok(ts) = &targets
        && !ts.is_empty()
    {
        sentence += &format!(" on {}", names(ts));
    }
    if spell.concentration {
        sentence += " (concentration)";
    }
    if let Some(problem) = problem {
        return (sentence, Err(problem));
    }
    let targets = match targets {
        Ok(ts) => ts,
        Err(e) => return (sentence, Err(e)),
    };

    let character = actor.character_id.expect("has a sheet");
    let action = |pact| CharacterAction::CastSpell {
        spell: spell.name.clone(),
        at_level,
        pact,
        ritual: false,
    };
    // Warlocks cast from pact slots; use them when that's what works.
    let action = match sheet.clone().apply(&action(pact)) {
        Ok(()) => action(pact),
        Err(_) if !pact && sheet.clone().apply(&action(true)).is_ok() => action(true),
        Err(e) => return (sentence, Err(e.to_string())),
    };
    let slot = match &action {
        _ if spell.level == 0 => None,
        CharacterAction::CastSpell { pact: true, .. } => sheet
            .spellcasting
            .pact_magic
            .map(|p| SpellSlot::Pact(p.slot_level)),
        _ => Some(SpellSlot::Level(at_level.unwrap_or(spell.level))),
    };
    // The slot is spent on the sheet first; the cast then goes in the log
    // and, if it needs concentration, into play.
    let cast = CombatCommand::CastSpell {
        caster: actor.id,
        spell: spell.name.clone(),
        slot,
        targets: targets.iter().map(|t| t.id).collect(),
        concentration: spell.concentration.then(Uuid::new_v4),
    };
    let steps = vec![
        Step::Character {
            id: character,
            action,
        },
        Step::Combat(cast),
    ];
    (sentence, Ok(steps))
}

/// `effect <name> [who…] [for <rounds>]`: targets are optional, for things
/// in the area like a fire on the floor.
fn effect(actor: Option<&Combatant>, args: &[&str], ctx: &Context) -> Interpreted {
    // The name runs up to the first target or `for`.
    let split = args
        .iter()
        .position(|w| w.parse::<usize>().is_ok() || w.eq_ignore_ascii_case("for"))
        .unwrap_or(args.len());
    let name = args[..split].join(" ");
    let mut target_words = Vec::new();
    let mut rounds = None;
    let mut problem = None;
    let mut rest = args[split..].iter();
    while let Some(word) = rest.next() {
        if word.eq_ignore_ascii_case("for") {
            match rest.next().map(|r| r.parse::<u32>()) {
                Some(Ok(r)) => rounds = Some(r),
                Some(Err(_)) | None => problem = Some("for how many rounds?".to_string()),
            }
        } else {
            target_words.push(*word);
        }
    }
    let targets = targets(&target_words, ctx);
    let shown_name = if name.is_empty() { "?" } else { &name };
    let mut sentence = match (actor, &targets) {
        (Some(a), Ok(ts)) if !ts.is_empty() => {
            format!("{} puts {shown_name} on {}", a.name, names(ts))
        }
        (Some(a), _) => format!("{} puts down {shown_name}", a.name),
        (None, Ok(ts)) if !ts.is_empty() => format!("{shown_name} on {}", names(ts)),
        (None, _) => shown_name.to_string(),
    };
    if let Some(r) = rounds {
        sentence += &format!(" for {r} round{}", if r == 1 { "" } else { "s" });
    }
    if name.is_empty() {
        return (sentence, Err("what effect? type its name".into()));
    }
    // Targets are optional, so a combatant's name typed where a target goes
    // would quietly become part of the effect's name.
    let looks_like_target = args[..split].iter().skip(1).find(|w| {
        w.len() >= 2
            && ctx
                .state
                .combatants
                .iter()
                .any(|c| c.name.to_lowercase().starts_with(&w.to_lowercase()))
    });
    if let Some(word) = looks_like_target {
        return (
            sentence,
            Err(format!("press Tab to pick \"{word}\" as a target")),
        );
    }
    if let Some(problem) = problem {
        return (sentence, Err(problem));
    }
    let targets = match targets {
        Ok(ts) => ts,
        Err(e) => return (sentence, Err(e)),
    };
    let effect = NewEffect {
        id: Uuid::new_v4(),
        name,
        source: actor.map(|a| a.id),
        targets: targets.iter().map(|t| t.id).collect(),
        concentration: false,
        rounds,
    };
    (
        sentence,
        Ok(vec![Step::Combat(CombatCommand::AddEffect { effect })]),
    )
}

/// `use`: only ever on oneself.
fn use_feature(actor: Option<&Combatant>, args: &[&str], ctx: &Context) -> Interpreted {
    let Some(actor) = actor else {
        return (
            "? uses …".into(),
            Err("who? start with their number".into()),
        );
    };
    let Some(sheet) = ctx.sheet(actor) else {
        return (
            format!("{} uses …", actor.name),
            Err(format!("{} has no character sheet", actor.name)),
        );
    };
    let resources = sheet.resources.iter().map(|r| r.name.as_str());
    let Some((name, used)) = longest_name(args, resources) else {
        let error = if args.is_empty() {
            "which feature? Tab lists them".to_string()
        } else {
            format!("{} has no \"{}\"", actor.name, args.join(" "))
        };
        return (format!("{} uses …", actor.name), Err(error));
    };
    let (amount, sentence) = match &args[used..] {
        [] => (1, format!("{} uses {name}", actor.name)),
        [n] => match n.parse::<u16>() {
            Ok(n) => (n, format!("{} uses {name} ×{n}", actor.name)),
            Err(_) => {
                return (
                    format!("{} uses {name}", actor.name),
                    Err("only on themselves; just the amount, if more than 1".into()),
                );
            }
        },
        _ => {
            return (
                format!("{} uses {name}", actor.name),
                Err("only on themselves; just the amount, if more than 1".into()),
            );
        }
    };
    let action = CharacterAction::UseResource {
        name: name.to_string(),
        amount,
    };
    if let Err(e) = sheet.clone().apply(&action) {
        return (sentence, Err(e.to_string()));
    }
    let id = actor.character_id.expect("has a sheet");
    let used = CombatCommand::UseFeature {
        combatant: actor.id,
        feature: name.to_string(),
        amount,
    };
    (
        sentence,
        Ok(vec![Step::Character { id, action }, Step::Combat(used)]),
    )
}

/// `add`: a campaign character, `[count] <monster> [roll]` from the
/// bestiary, or anyone else as `<name> <hp>`.
fn add(args: &[&str], ctx: &Context) -> Interpreted {
    let text = args.join(" ");
    if text.is_empty() {
        return (
            "? joins the combat".into(),
            Err("who? a character, a monster, or a name and hit points".into()),
        );
    }
    if let Some(character) = ctx
        .roster
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(&text))
    {
        let sentence = format!("{} joins the combat", character.name);
        let present = ctx
            .state
            .combatants
            .iter()
            .any(|c| c.character_id == Some(character.id));
        if present {
            return (
                sentence,
                Err(format!("{} is already in it", character.name)),
            );
        }
        return (sentence, Ok(vec![Step::Join(character.id)]));
    }

    let (count, rest) = match args.split_first() {
        Some((first, rest)) if !rest.is_empty() => match first.parse::<u16>() {
            Ok(n) => (Some(n), rest),
            Err(_) => (None, args),
        },
        _ => (None, args),
    };
    let (roll, rest) = match rest.split_last() {
        Some((last, init)) if last.eq_ignore_ascii_case("roll") && !init.is_empty() => (true, init),
        _ => (false, rest),
    };
    let wanted = rest.join(" ");
    if let Some(monster) = monster_named(ctx.bestiary, &wanted) {
        let count = count.unwrap_or(1);
        if count == 0 {
            return ("Nobody joins".into(), Err("how many? at least 1".into()));
        }
        let names = monster_names(&monster.name, count, ctx.state);
        let who = match names.as_slice() {
            [init @ .., last] if !init.is_empty() => format!("{} and {last}", init.join(", ")),
            _ => names.join(""),
        };
        let verb = if count == 1 { "joins" } else { "join" };
        let hp = match (roll, count) {
            (true, _) => "with rolled hit points".to_string(),
            (false, 1) => format!("at {} HP", monster.max_hit_points),
            (false, _) => format!("at {} HP each", monster.max_hit_points),
        };
        return (
            format!("{who} {verb} the combat {hp}"),
            Ok(vec![Step::Summon {
                monster: monster.id,
                count,
                roll,
            }]),
        );
    }
    if count.is_some() || roll {
        return (
            format!("{wanted} joins the combat"),
            Err(format!("no monster called \"{wanted}\"; Tab lists them")),
        );
    }

    let (name, hp) = match args.split_last() {
        Some((last, name)) if !name.is_empty() => match last.parse::<u16>() {
            Ok(hp) => (name.join(" "), Some(hp)),
            Err(_) => (text.clone(), None),
        },
        _ => (text.clone(), None),
    };
    match hp {
        Some(hp) => (
            format!("{name} ({hp} HP) joins the combat"),
            Ok(vec![Step::Combat(CombatCommand::AddCombatant {
                combatant: Combatant::new(name, hp),
            })]),
        ),
        None => (
            format!("{name} joins the combat"),
            Err(format!(
                "not a character or monster; give hit points to add anyone, e.g. add {name} 7"
            )),
        ),
    }
}

/// The monster called `name` (any case), the campaign's own before a
/// shared one.
pub fn monster_named<'m>(bestiary: &'m [MonsterSummary], name: &str) -> Option<&'m MonsterSummary> {
    let mut matching: Vec<_> = bestiary
        .iter()
        .filter(|m| m.name.eq_ignore_ascii_case(name))
        .collect();
    matching.sort_by_key(|m| m.shared);
    matching.first().copied()
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

/// The combatants that target words refer to.
fn targets<'c>(words: &[&str], ctx: &'c Context) -> Result<Vec<&'c Combatant>, String> {
    words.iter().map(|w| ctx.target(w)).collect()
}

/// The longest name in `names` that `words` start with, and how many words
/// it took.
fn longest_name<'n>(
    words: &[&str],
    names: impl Iterator<Item = &'n str>,
) -> Option<(&'n str, usize)> {
    names
        .filter_map(|name| {
            let parts: Vec<&str> = name.split_whitespace().collect();
            let matches = parts.len() <= words.len()
                && parts
                    .iter()
                    .zip(words)
                    .all(|(p, w)| p.eq_ignore_ascii_case(w));
            matches.then_some((name, parts.len()))
        })
        .max_by_key(|(_, used)| *used)
}

/// "Vex", "Vex and Grog", "Vex, Grog and Pike".
fn names(combatants: &[&Combatant]) -> String {
    match combatants {
        [] => String::new(),
        [one] => one.name.clone(),
        [init @ .., last] => {
            let init: Vec<_> = init.iter().map(|c| c.name.as_str()).collect();
            format!("{} and {}", init.join(", "), last.name)
        }
    }
}

fn verb_s<'a>(combatants: &[&Combatant], one: &'a str, many: &'a str) -> &'a str {
    if combatants.len() == 1 { one } else { many }
}

/// Catches combat commands that would certainly be refused, before sending.
fn check(steps: Vec<Step>, ctx: &Context) -> Result<Vec<Step>, String> {
    let mut state = ctx.state.clone();
    for step in &steps {
        if let Step::Combat(command) = step {
            match decide(&state, command, &mut NoDice) {
                Ok(events) => events.iter().for_each(|e| state.apply(e)),
                Err(CombatError::DiceUnavailable | CombatError::ServerOnly) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    Ok(steps)
}

/// What could come next on `line`: actions, combatants (completed to their
/// number), spells, features, effects or characters, depending on where the
/// cursor is.
pub fn suggest(line: &str, ctx: &Context) -> Vec<Suggestion> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let typing = !line.is_empty() && !line.ends_with(char::is_whitespace);
    let (done, partial) = match (typing, words.split_last()) {
        (true, Some((last, done))) => (done, *last),
        _ => (&words[..], ""),
    };
    let head = |extra: &[&str]| {
        let mut parts: Vec<&str> = done.to_vec();
        parts.extend_from_slice(extra);
        parts.join(" ")
    };
    let complete = |base: &str, word: &str| {
        if base.is_empty() {
            format!("{word} ")
        } else {
            format!("{base} {word} ")
        }
    };

    // Where the action word is: after an optional leading number.
    let verb_at = match done.first() {
        Some(w) if w.parse::<usize>().is_ok() => 1,
        _ => 0,
    };
    if done.len() <= verb_at {
        let mut out: Vec<Suggestion> = VERBS
            .iter()
            .filter(|(_, name, ..)| name.starts_with(&partial.to_lowercase()))
            .map(|(_, name, args, about)| Suggestion {
                line: complete(&head(&[]), name),
                label: format!("{name} {args}").trim_end().to_string(),
                detail: about.to_string(),
            })
            .collect();
        if done.is_empty() && !partial.is_empty() {
            // Starting with someone's name picks who acts.
            out.extend(combatants(ctx, partial).map(|(n, c)| Suggestion {
                line: format!("{n} "),
                label: format!("{n} {}", c.name),
                detail: "acts".into(),
            }));
        }
        return out;
    }

    let Some(verb) = verb(done[verb_at]) else {
        return Vec::new();
    };
    let args = &done[verb_at + 1..];
    let base = head(&[]);
    let actor = match verb_at {
        1 => done[0].parse().ok().and_then(|n| ctx.numbered(n)),
        _ => ctx.state.current_turn.and_then(|id| ctx.by_id(id)),
    };
    let target_suggestions = || -> Vec<Suggestion> {
        combatants(ctx, partial)
            .map(|(n, c)| Suggestion {
                line: complete(&base, &n.to_string()),
                label: format!("{n} {}", c.name),
                detail: status(c),
            })
            .collect()
    };
    // Names that may span several words: offered while what's typed after
    // the action could still be the start of one.
    // `skip` words after the action come before the name (a count).
    let named_after = |skip: usize, names: Vec<(String, String)>| -> Vec<Suggestion> {
        let mut typed: Vec<&str> = args[skip..].to_vec();
        if !partial.is_empty() {
            typed.push(partial);
        }
        let typed = typed.join(" ").to_lowercase();
        let verb_head = done[..=verb_at + skip].join(" ");
        names
            .into_iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&typed))
            .filter(|(name, _)| !(partial.is_empty() && name.eq_ignore_ascii_case(&typed)))
            .map(|(name, detail)| Suggestion {
                line: format!("{verb_head} {name} "),
                label: name,
                detail,
            })
            .collect()
    };
    let named = |names| named_after(0, names);

    match verb {
        Verb::Damage | Verb::Heal | Verb::Temp | Verb::Remove => target_suggestions(),
        Verb::Init if args.is_empty() => target_suggestions(),
        // First the effect's name, then whom it's on. Once there's a name,
        // every target is offered, the first included, along with longer
        // names it could still be the start of.
        Verb::Effect if args.is_empty() => named(effect_names(ctx)),
        Verb::Effect => {
            let timed = args.iter().any(|w| w.eq_ignore_ascii_case("for"));
            if args.last().is_some_and(|w| w.eq_ignore_ascii_case("for")) {
                return Vec::new(); // the number of rounds comes next
            }
            let mut out = if timed {
                Vec::new()
            } else {
                target_suggestions()
            };
            if !args.iter().any(|w| w.parse::<usize>().is_ok()) && !timed {
                out.extend(named(effect_names(ctx)));
            }
            if !timed && "for".starts_with(&partial.to_lowercase()) {
                out.push(Suggestion {
                    line: complete(&base, "for"),
                    label: "for <rounds>".into(),
                    detail: "how long it lasts; leave out for until removed".into(),
                });
            }
            out
        }
        Verb::Cast => {
            let Some(sheet) = actor.and_then(|a| ctx.sheet(a)) else {
                return Vec::new();
            };
            let spells = &sheet.spellcasting.spells;
            match longest_name(args, spells.iter().map(|s| s.name.as_str())) {
                Some(_) if !partial.eq_ignore_ascii_case("at") => target_suggestions(),
                Some(_) => Vec::new(),
                None => named(
                    spells
                        .iter()
                        .filter(|s| s.prepared || s.ritual)
                        .map(|s| (s.name.clone(), spell_detail(s, sheet)))
                        .collect(),
                ),
            }
        }
        Verb::Use => {
            let Some(sheet) = actor.and_then(|a| ctx.sheet(a)) else {
                return Vec::new();
            };
            named(
                sheet
                    .resources
                    .iter()
                    .map(|r| (r.name.clone(), crate::display::pool(r.uses)))
                    .collect(),
            )
        }
        Verb::Drop => named(
            ctx.state
                .effects
                .iter()
                .map(|e| {
                    let on: Vec<&str> = e
                        .targets
                        .iter()
                        .filter_map(|t| ctx.by_id(*t))
                        .map(|c| c.name.as_str())
                        .collect();
                    let detail = if on.is_empty() {
                        String::new()
                    } else {
                        format!("on {}", on.join(", "))
                    };
                    (e.name.clone(), detail)
                })
                .collect(),
        ),
        Verb::Add => {
            let counted = args.first().is_some_and(|w| w.parse::<u16>().is_ok());
            let skip = usize::from(counted);
            // After a whole monster's name: whether to roll its hit points.
            if monster_named(ctx.bestiary, &args[skip..].join(" ")).is_some() {
                return if "roll".starts_with(&partial.to_lowercase()) {
                    vec![Suggestion {
                        line: complete(&base, "roll"),
                        label: "roll".into(),
                        detail: "roll hit points instead of taking the maximum".into(),
                    }]
                } else {
                    Vec::new()
                };
            }
            let characters = ctx
                .roster
                .iter()
                .filter(|_| !counted)
                .filter(|r| {
                    !ctx.state
                        .combatants
                        .iter()
                        .any(|c| c.character_id == Some(r.id))
                })
                .map(|r| (r.name.clone(), format!("level {} {}", r.level, r.race)));
            let monsters = ctx.bestiary.iter().map(|m| {
                let whose = if m.shared { "" } else { " · campaign's own" };
                let detail = format!(
                    "CR {} · {} {} · {} HP{whose}",
                    m.challenge, m.size, m.kind, m.max_hit_points
                );
                (m.name.clone(), detail)
            });
            let mut names: Vec<(String, String)> = characters.chain(monsters).collect();
            names.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
            named_after(skip, names)
        }
        _ => Vec::new(),
    }
}

/// The conditions, then the effects in play (in case one goes on someone
/// else too), with what they are.
fn effect_names(ctx: &Context) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = CONDITIONS
        .iter()
        .map(|c| (c.to_string(), "condition".to_string()))
        .collect();
    for effect in &ctx.state.effects {
        if !names
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(&effect.name))
        {
            names.push((effect.name.clone(), "in play".into()));
        }
    }
    names
}

/// The conditions of the rules (the same in both editions).
const CONDITIONS: &[&str] = &[
    "Blinded",
    "Charmed",
    "Deafened",
    "Exhaustion",
    "Frightened",
    "Grappled",
    "Incapacitated",
    "Invisible",
    "Paralyzed",
    "Petrified",
    "Poisoned",
    "Prone",
    "Restrained",
    "Stunned",
    "Unconscious",
];

/// Combatants with their numbers whose name or number starts with `typed`.
fn combatants<'c>(
    ctx: &'c Context,
    typed: &str,
) -> impl Iterator<Item = (usize, &'c Combatant)> + use<'c> {
    let typed = typed.to_lowercase();
    ctx.state
        .combatants
        .iter()
        .enumerate()
        .map(|(i, c)| (i + 1, c))
        .filter(move |(n, c)| {
            typed.is_empty()
                || c.name.to_lowercase().starts_with(&typed)
                || n.to_string().starts_with(&typed)
        })
}

fn status(c: &Combatant) -> String {
    let hp = c.hit_points;
    let temp = if hp.temporary > 0 {
        format!(" +{}", hp.temporary)
    } else {
        String::new()
    };
    format!("{}/{}{temp} HP", hp.current, hp.max)
}

fn spell_detail(spell: &Spell, sheet: &CharacterSheet) -> String {
    let mut parts = vec![if spell.level == 0 {
        "cantrip".to_string()
    } else {
        format!("{} level", crate::display::ordinal(spell.level))
    }];
    if spell.concentration {
        parts.push("concentration".into());
    }
    if spell.level > 0 {
        let left: u16 = sheet
            .spellcasting
            .slots
            .range(spell.level..)
            .map(|(_, p)| p.remaining())
            .sum::<u16>()
            + sheet
                .spellcasting
                .pact_magic
                .filter(|p| p.slot_level >= spell.level)
                .map_or(0, |p| p.slots.remaining());
        parts.push(format!(
            "{left} slot{} left",
            if left == 1 { "" } else { "s" }
        ));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use campaign_types::{
        Pool, Resource,
        combat::{CombatEvent, Effect},
    };
    use chrono::Utc;

    use super::*;

    struct World {
        state: CombatState,
        sheets: HashMap<Uuid, Character>,
        roster: Vec<CharacterSummary>,
        bestiary: Vec<MonsterSummary>,
    }

    fn monster(name: &str, max_hit_points: u16, shared: bool) -> MonsterSummary {
        MonsterSummary {
            id: Uuid::new_v4(),
            name: name.into(),
            size: "Small".into(),
            kind: "humanoid".into(),
            challenge: "1/4".into(),
            hit_points: 7,
            max_hit_points,
            xp: 50,
            armor_class: 15,
            source: if shared { "SRD 5.2" } else { "Homebrew" }.into(),
            shared,
        }
    }

    fn character(sheet: CharacterSheet) -> Character {
        Character {
            id: Uuid::new_v4(),
            campaign: Uuid::nil(),
            sheet,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// Vex (a ranger with spells and a feature) has the turn; then Grog,
    /// a goblin and Pike, who's in the campaign but not the fight.
    fn world() -> World {
        let mut vex = CharacterSheet {
            name: "Vex".into(),
            ..Default::default()
        };
        let spell = |name: &str, level, concentration| Spell {
            name: name.into(),
            level,
            class: None,
            prepared: true,
            concentration,
            ritual: false,
            notes: String::new(),
        };
        vex.spellcasting.spells = vec![
            spell("Hunter's Mark", 1, true),
            spell("Cure Wounds", 1, false),
            spell("Cure Wounds Mass", 5, false),
        ];
        vex.spellcasting.slots.insert(1, Pool::new(2));
        vex.spellcasting.slots.insert(2, Pool::new(1));
        vex.resources.push(Resource {
            name: "Favored Foe".into(),
            uses: Pool::new(3),
            recharge: Default::default(),
            notes: String::new(),
        });
        let vex = character(vex);
        let pike = character(CharacterSheet {
            name: "Pike".into(),
            ..Default::default()
        });
        let mut vex_c = Combatant::from_character(&vex);
        vex_c.initiative = Some(20);
        let mut grog = Combatant::new("Grog", 50);
        grog.initiative = Some(15);
        let mut goblin = Combatant::new("Goblin", 7);
        goblin.initiative = Some(10);
        let mut state = CombatState::default();
        for e in [
            CombatEvent::CombatantAdded { combatant: vex_c },
            CombatEvent::CombatantAdded { combatant: grog },
            CombatEvent::CombatantAdded { combatant: goblin },
            CombatEvent::Started,
            CombatEvent::RoundStarted { round: 1 },
        ] {
            state.apply(&e);
        }
        let first = state.combatants[0].id;
        state.apply(&CombatEvent::TurnStarted { combatant: first });
        World {
            state,
            roster: vec![(&vex).into(), (&pike).into()],
            sheets: HashMap::from([(vex.id, vex)]),
            bestiary: vec![
                monster("Cave Troll", 90, true),
                monster("Cave Troll", 120, false),
                monster("Goblin", 12, true),
                monster("Goblin Boss", 30, true),
            ],
        }
    }

    impl World {
        fn ctx(&self) -> Context<'_> {
            Context {
                state: &self.state,
                sheets: &self.sheets,
                roster: &self.roster,
                bestiary: &self.bestiary,
            }
        }

        fn parse(&self, line: &str) -> Parsed {
            parse(line, &self.ctx())
        }

        fn id(&self, n: usize) -> Uuid {
            self.state.combatants[n - 1].id
        }

        fn lines(&self, line: &str) -> Vec<String> {
            suggest(line, &self.ctx())
                .into_iter()
                .map(|s| s.line)
                .collect()
        }
    }

    #[test]
    fn the_turn_holder_acts_unless_someone_else_is_named() {
        let w = world();
        let p = w.parse("damage 3 7");
        assert_eq!(p.sentence, "Vex deals 7 damage to Goblin");
        assert_eq!(p.actor, Some(w.id(1)));
        assert_eq!(
            p.plan,
            Ok(vec![Step::Combat(CombatCommand::Damage {
                combatant: w.id(3),
                amount: 7
            })])
        );
        let p = w.parse("2 dam 3 7");
        assert_eq!(p.sentence, "Grog deals 7 damage to Goblin");
        assert_eq!(p.actor, Some(w.id(2)));
    }

    #[test]
    fn the_sentence_fills_in_as_you_type() {
        let w = world();
        assert_eq!(w.parse("2").sentence, "Grog …");
        let p = w.parse("heal 2");
        assert_eq!(p.sentence, "Vex heals Grog for ?");
        assert_eq!(p.plan, Err("how much? type the amount".into()));
        let p = w.parse("damage 2 3 8");
        assert_eq!(p.sentence, "Vex deals 8 damage to Grog and Goblin");
        assert_eq!(p.plan.unwrap().len(), 2);
        assert_eq!(
            w.parse("damage gob").plan,
            Err("press Tab to pick \"gob\"".into())
        );
        assert_eq!(
            w.parse("damage 9 1").plan,
            Err("there's no number 9".into())
        );
    }

    #[test]
    fn spells_spend_the_casters_slot_and_concentration_becomes_an_effect() {
        let w = world();
        let p = w.parse("cast hunter's mark 3");
        assert_eq!(
            p.sentence,
            "Vex casts Hunter's Mark on Goblin (concentration)"
        );
        let steps = p.plan.unwrap();
        let vex = w.state.combatants[0].character_id.unwrap();
        assert!(matches!(
            &steps[0],
            Step::Character { id, action: CharacterAction::CastSpell { spell, .. } }
                if *id == vex && spell == "Hunter's Mark"
        ));
        assert!(matches!(
            &steps[1],
            Step::Combat(CombatCommand::CastSpell { caster, slot: Some(SpellSlot::Level(1)), targets, concentration: Some(_), .. })
                if *caster == w.id(1) && *targets == [w.id(3)]
        ));

        // The longest matching name wins; no effect without concentration.
        let p = w.parse("cast cure wounds mass 2 3");
        assert_eq!(p.sentence, "Vex casts Cure Wounds Mass on Grog and Goblin");
        assert_eq!(p.plan, Err("no level 5 spell slots".into()));
        let steps = w.parse("cast cure wounds 2 at 2").plan.unwrap();
        assert!(matches!(
            &steps[1],
            Step::Combat(CombatCommand::CastSpell {
                slot: Some(SpellSlot::Level(2)),
                concentration: None,
                ..
            })
        ));
        assert_eq!(
            w.parse("2 cast cure wounds").plan,
            Err("Grog has no character sheet".into())
        );
    }

    #[test]
    fn features_are_used_on_oneself_only() {
        let w = world();
        let p = w.parse("use favored foe 2");
        assert_eq!(p.sentence, "Vex uses Favored Foe ×2");
        assert!(matches!(
            &p.plan.unwrap()[1],
            Step::Combat(CombatCommand::UseFeature { combatant, amount: 2, .. }) if *combatant == w.id(1)
        ));
        assert!(w.parse("use favored foe 3 1").plan.is_err());
        assert_eq!(
            w.parse("use favored foe 4").plan,
            Err("Favored Foe has 3 uses remaining, 4 requested".into())
        );
    }

    #[test]
    fn effects_initiative_and_joining() {
        let mut w = world();
        let p = w.parse("effect Prone 3 for 2");
        assert_eq!(p.sentence, "Vex puts Prone on Goblin for 2 rounds");
        assert!(p.plan.is_ok());
        assert_eq!(w.parse("init 3 17").sentence, "Goblin's initiative is 17");
        assert_eq!(
            w.parse("add pike").plan,
            Ok(vec![Step::Join(w.roster[1].id)])
        );
        assert_eq!(w.parse("add vex").plan, Err("Vex is already in it".into()));
        assert_eq!(
            w.parse("add Goblin Archer 12").sentence,
            "Goblin Archer (12 HP) joins the combat"
        );

        let bless = Effect {
            id: Uuid::new_v4(),
            name: "Bless".into(),
            source: None,
            targets: vec![w.id(2)],
            concentration: false,
            expires: None,
        };
        w.state.apply(&CombatEvent::EffectAdded {
            effect: bless.clone(),
        });
        assert_eq!(
            w.parse("drop bless").plan,
            Ok(vec![Step::Combat(CombatCommand::RemoveEffect {
                effect: bless.id
            })])
        );
    }

    #[test]
    fn certainly_refused_commands_are_caught_before_sending() {
        let w = world();
        assert_eq!(
            w.parse("start").plan,
            Err("combat has already started".into())
        );
        assert!(w.parse("next").plan.is_ok());
        assert_eq!(w.parse("next").sentence, "Vex's turn ends");
        assert!(w.parse("undo").plan.is_ok(), "the server decides undos");
    }

    #[test]
    fn suggestions_complete_actions_names_and_numbers() {
        let w = world();
        let all = suggest("", &w.ctx());
        assert_eq!(all.len(), VERBS.len(), "every action, with its usage");
        assert_eq!(w.lines("he"), ["heal "]);
        // A name first picks who acts, by number.
        assert_eq!(w.lines("gr"), ["2 "]);
        assert_eq!(w.lines("2 ca"), ["2 cast "]);
        // Targets complete to their numbers.
        assert_eq!(w.lines("damage gob"), ["damage 3 "]);
        assert_eq!(w.lines("damage ").len(), 3);
        // Multi-word names complete whole.
        assert_eq!(
            w.lines("cast cure"),
            ["cast Cure Wounds ", "cast Cure Wounds Mass "]
        );
        assert_eq!(w.lines("cast hunter's mark ").len(), 3, "then targets");
        assert_eq!(w.lines("use f"), ["use Favored Foe "]);
        assert_eq!(w.lines("add p"), ["add Pike "]);
        assert!(
            !w.lines("add ").contains(&"add Vex ".to_string()),
            "Vex is already in"
        );
    }

    #[test]
    fn monsters_join_numbered_at_their_maximum_or_rolled() {
        // The fight already has a plain "Goblin" (number 3).
        let w = world();
        let goblin = w.bestiary[2].id;
        let p = w.parse("add 2 goblin");
        assert_eq!(
            p.sentence,
            "Goblin 2 and Goblin 3 join the combat at 12 HP each"
        );
        assert_eq!(
            p.plan,
            Ok(vec![Step::Summon {
                monster: goblin,
                count: 2,
                roll: false
            }])
        );
        let p = w.parse("add goblin boss roll");
        assert_eq!(
            p.sentence,
            "Goblin Boss joins the combat with rolled hit points"
        );
        assert!(matches!(
            p.plan.unwrap()[0],
            Step::Summon {
                roll: true,
                count: 1,
                ..
            }
        ));
        let own_troll = w.bestiary[1].id;
        assert!(
            matches!(
                w.parse("add cave troll").plan.unwrap()[0],
                Step::Summon { monster, .. } if monster == own_troll
            ),
            "the campaign's own before the shared one"
        );
        assert_eq!(
            w.parse("add 2 dragon").plan,
            Err("no monster called \"dragon\"; Tab lists them".into())
        );
        assert!(
            matches!(w.parse("add Goblin 7").plan.unwrap()[0], Step::Combat(_)),
            "a name and hit points still adds anyone"
        );

        assert_eq!(
            w.lines("add 3 gob"),
            ["add 3 Goblin ", "add 3 Goblin Boss "]
        );
        assert!(
            w.lines("add 3 p").is_empty(),
            "characters come one at a time"
        );
        assert_eq!(w.lines("add goblin "), ["add goblin roll "]);
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

    #[test]
    fn effects_need_no_target_and_last_for_rounds_of_whoever_put_them_down() {
        let w = world();
        let p = w.parse("effect Fire for 3");
        assert_eq!(p.sentence, "Vex puts down Fire for 3 rounds");
        let Ok(steps) = p.plan else {
            panic!("{:?}", p.plan)
        };
        assert!(matches!(
            &steps[0],
            Step::Combat(CombatCommand::AddEffect { effect })
                if effect.name == "Fire" && effect.targets.is_empty()
                    && effect.rounds == Some(3) && effect.source == Some(w.id(1))
        ));
        let p = w.parse("2 effect Wall of Fire");
        assert_eq!(p.sentence, "Grog puts down Wall of Fire");
        assert!(p.plan.is_ok(), "until removed");
        assert_eq!(
            w.parse("effect Bless 2 for 10").sentence,
            "Vex puts Bless on Grog for 10 rounds"
        );
        assert_eq!(w.lines("effect Fire f"), ["effect Fire for "]);
        assert!(
            w.lines("effect Fire for ").is_empty(),
            "the rounds come next"
        );
    }

    #[test]
    fn effects_complete_their_name_and_every_target_including_the_first() {
        // Regression: combatants were only offered once a first number had
        // been typed, so the first target got no help, and a name typed
        // there became part of the effect's name.
        let mut w = world();
        assert_eq!(w.lines("effect pr"), ["effect Prone "]);
        assert_eq!(suggest("effect ", &w.ctx()).len(), CONDITIONS.len());
        assert_eq!(
            w.lines("effect Prone "),
            [
                "effect Prone 1 ",
                "effect Prone 2 ",
                "effect Prone 3 ",
                "effect Prone for "
            ]
        );
        assert_eq!(w.lines("effect Prone gob"), ["effect Prone 3 "]);
        assert_eq!(
            w.parse("effect Prone gob").plan,
            Err("press Tab to pick \"gob\" as a target".into())
        );
        assert_eq!(w.lines("effect Prone 3 gr"), ["effect Prone 3 2 "]);

        // Effects already in play are offered as names, also partway.
        w.state.apply(&CombatEvent::EffectAdded {
            effect: Effect {
                id: Uuid::new_v4(),
                name: "Hunter's Mark".into(),
                source: None,
                targets: vec![],
                concentration: false,
                expires: None,
            },
        });
        assert_eq!(w.lines("effect hu"), ["effect Hunter's Mark "]);
        assert_eq!(w.lines("effect hunter's m"), ["effect Hunter's Mark "]);
    }
}
