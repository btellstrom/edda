//! What each action means: the sentence it reads as, and the steps it
//! takes.

use campaign_types::{
    CharacterAction,
    combat::{CombatCommand, Combatant, NewEffect, SpellSlot, monster_names},
};
use uuid::Uuid;

use super::{
    Context, Step, Topic, VERBS, Verb,
    names::{longest_name, names, targets, verb_s},
};
use crate::{display::Section, session::monster_named};

/// The part of the actor's sheet that doing `verb` needs: a monster's
/// actions for anything it does to others, a character's spells or
/// features.
pub(super) fn section(verb: Verb, actor: Option<&Combatant>) -> Option<Section> {
    let monster = actor.is_some_and(|a| a.monster_id.is_some());
    match verb {
        Verb::Damage | Verb::Heal | Verb::Temp | Verb::Cast | Verb::Effect | Verb::Use
            if monster =>
        {
            Some(Section::Actions)
        }
        Verb::Cast => Some(Section::Spellcasting),
        Verb::Use => Some(Section::Features),
        _ => None,
    }
}

/// What a line with `verb` is about: the spell cast, the effect put down
/// or ended, or what's looked at.
pub(super) fn topic(
    verb: Verb,
    actor: Option<&Combatant>,
    args: &[&str],
    ctx: &Context,
) -> Option<Topic> {
    if args.is_empty() {
        return None;
    }
    // Names run up to the first number (a target), or `for` or `at`.
    let end = args
        .iter()
        .position(|w| {
            w.parse::<usize>().is_ok() || ["for", "at"].contains(&w.to_lowercase().as_str())
        })
        .unwrap_or(args.len());
    let name = args[..end].join(" ");
    match verb {
        Verb::Cast => {
            let known = actor.and_then(|a| ctx.sheet(a)).and_then(|sheet| {
                longest_name(
                    args,
                    sheet.spellcasting.spells.iter().map(|s| s.name.as_str()),
                )
            });
            Some(Topic::Spell(known.map_or(name, |(n, _)| n.to_string())))
        }
        Verb::Effect if !name.is_empty() => Some(Topic::Effect(name)),
        Verb::Drop => Some(Topic::Effect(args.join(" "))),
        Verb::Show => match args {
            [n] if n.parse::<usize>().is_ok() => n
                .parse()
                .ok()
                .and_then(|n| ctx.numbered(n))
                .map(|c| Topic::Combatant(c.id)),
            _ => Some(Topic::Named(args.join(" "))),
        },
        _ => None,
    }
}

/// The verb a word names, also by a unique prefix.
pub(super) fn verb(word: &str) -> Option<Verb> {
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

pub(super) fn interpret(
    verb: Verb,
    actor: Option<&Combatant>,
    args: &[&str],
    ctx: &Context,
) -> Interpreted {
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
        Verb::Show => show(args, ctx),
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
    let amount_value = amount.map(str::parse::<u32>);
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

fn show(args: &[&str], ctx: &Context) -> Interpreted {
    let typed = args.join(" ");
    if typed.is_empty() {
        return (
            "Show …".into(),
            Err("at what? a spell, effect, condition or combatant; Tab lists them".into()),
        );
    }
    let topic = match args {
        [n] if n.parse::<usize>().is_ok() => match n.parse().ok().and_then(|n| ctx.numbered(n)) {
            Some(c) => Topic::Combatant(c.id),
            None => {
                return (
                    format!("Show {typed}"),
                    Err(format!("there's no number {n}")),
                );
            }
        },
        _ => Topic::Named(typed.clone()),
    };
    let shown = match &topic {
        Topic::Combatant(id) => ctx.by_id(*id).map_or(typed.clone(), |c| c.name.clone()),
        _ => typed.clone(),
    };
    if ctx.describes(&topic) {
        (format!("Show {shown}"), Ok(vec![Step::Show(topic)]))
    } else {
        (
            format!("Show {shown}"),
            Err(format!(
                "nothing called \"{typed}\" is described; Tab lists what is"
            )),
        )
    }
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

    // Checked against the sheet here only to say what's wrong before
    // sending; the combat service spends the slot.
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
    // The cast goes in the log and, if it needs concentration, into play.
    let cast = CombatCommand::CastSpell {
        caster: actor.id,
        spell: spell.name.clone(),
        slot,
        targets: targets.iter().map(|t| t.id).collect(),
        concentration: spell.concentration.then(Uuid::new_v4),
    };
    (sentence, Ok(vec![Step::Combat(cast)]))
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
    let used = CombatCommand::UseFeature {
        combatant: actor.id,
        feature: name.to_string(),
        amount,
    };
    (sentence, Ok(vec![Step::Combat(used)]))
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
            Ok(vec![Step::Combat(CombatCommand::AddMonsters {
                monster: monster.id,
                count,
                roll,
            })]),
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
