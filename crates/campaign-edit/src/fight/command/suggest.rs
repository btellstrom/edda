//! What could come next on a line.

use campaign_types::{CharacterSheet, Spell, combat::Combatant};

use super::{Context, VERBS, Verb, names::longest_name, verb};
use crate::{prompt::Suggestion, session::monster_named};

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
        Verb::Show if args.iter().any(|w| w.parse::<usize>().is_ok()) => Vec::new(),
        Verb::Show => {
            let mut out = if args.is_empty() {
                target_suggestions()
            } else {
                Vec::new()
            };
            out.extend(named(show_names(ctx)));
            out
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

/// Everything `show` can describe: the effects in play, the conditions and
/// the spells (the campaign's, and any on a fighter's sheet).
fn show_names(ctx: &Context) -> Vec<(String, String)> {
    let mut names: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, detail: String| {
        if !names.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            names.push((name.to_string(), detail));
        }
    };
    for effect in &ctx.state.effects {
        add(&effect.name, "in play".into());
    }
    for condition in ctx.conditions {
        add(&condition.text.name, "condition".into());
    }
    for name in CONDITIONS {
        add(name, "condition".into());
    }
    for spell in ctx.spells {
        add(&spell.text.name, crate::display::spell_kind(&spell.text));
    }
    for sheet in ctx.state.combatants.iter().filter_map(|c| ctx.sheet(c)) {
        for spell in &sheet.spellcasting.spells {
            add(&spell.name, "spell".into());
        }
    }
    names
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
pub(super) const CONDITIONS: &[&str] = &[
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
