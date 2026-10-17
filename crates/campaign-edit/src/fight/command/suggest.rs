//! What could come next on a line.

use campaign_types::{CharacterSheet, Spell, combat::Combatant};

use super::{Context, VERBS, Verb, names::longest_name, verb};
use crate::{prompt::Suggestion, session::monster_named};

/// What could come next on `line`: actions, combatants (completed to their
/// number), spells, features, effects or characters, depending on where the
/// cursor is.
pub fn suggest(line: &str, ctx: &Context) -> Vec<Suggestion> {
    let typed = Typed::new(line, ctx);
    if typed.done.len() <= typed.verb_at {
        return typed.actions();
    }
    let Some(verb) = verb(typed.done[typed.verb_at]) else {
        return Vec::new();
    };
    match verb {
        Verb::Damage | Verb::Heal | Verb::Temp | Verb::Remove => typed.targets(),
        Verb::Init if typed.args().is_empty() => typed.targets(),
        Verb::Effect => typed.effect(),
        Verb::Cast => typed.cast(),
        Verb::Use => typed.use_feature(),
        Verb::Show => typed.show(),
        Verb::Drop => typed.named(drop_names(ctx)),
        Verb::Add => typed.add(),
        _ => Vec::new(),
    }
}

/// A line split up for completing it.
struct Typed<'l, 'c> {
    ctx: &'l Context<'c>,
    /// The words finished so far.
    done: Vec<&'l str>,
    /// The word being typed, or "" after a space.
    partial: &'l str,
    /// Where the action word is: after an optional leading number.
    verb_at: usize,
}

impl<'l, 'c> Typed<'l, 'c> {
    fn new(line: &'l str, ctx: &'l Context<'c>) -> Self {
        let words: Vec<&str> = line.split_whitespace().collect();
        let typing = !line.is_empty() && !line.ends_with(char::is_whitespace);
        let (done, partial) = match (typing, words.split_last()) {
            (true, Some((last, done))) => (done.to_vec(), *last),
            _ => (words, ""),
        };
        let verb_at = match done.first() {
            Some(w) if w.parse::<usize>().is_ok() => 1,
            _ => 0,
        };
        Self {
            ctx,
            done,
            partial,
            verb_at,
        }
    }

    /// The words after the action.
    fn args(&self) -> &[&'l str] {
        &self.done[self.verb_at + 1..]
    }

    /// The line with `word` in place of the one being typed.
    fn complete(&self, word: &str) -> String {
        if self.done.is_empty() {
            format!("{word} ")
        } else {
            format!("{} {word} ", self.done.join(" "))
        }
    }

    /// `word` (shown as `label`), if what's being typed could be its start.
    fn keyword(&self, word: &str, label: &str, detail: &str) -> Option<Suggestion> {
        word.starts_with(&self.partial.to_lowercase())
            .then(|| Suggestion {
                line: self.complete(word),
                label: label.into(),
                detail: detail.into(),
            })
    }

    /// Who acts: the line's leading number, or whoever has the turn.
    fn actor_sheet(&self) -> Option<&'l CharacterSheet> {
        let ctx = self.ctx;
        let actor = match self.verb_at {
            1 => self.done[0].parse().ok().and_then(|n| ctx.numbered(n)),
            _ => ctx.state.current_turn.and_then(|id| ctx.by_id(id)),
        };
        actor.and_then(|a| ctx.sheet(a))
    }

    /// The actions, and on a line's first word, who could act.
    fn actions(&self) -> Vec<Suggestion> {
        let mut out: Vec<Suggestion> = VERBS
            .iter()
            .filter(|(_, name, ..)| name.starts_with(&self.partial.to_lowercase()))
            .map(|(_, name, args, about)| Suggestion {
                line: self.complete(name),
                label: format!("{name} {args}").trim_end().to_string(),
                detail: about.to_string(),
            })
            .collect();
        if self.done.is_empty() && !self.partial.is_empty() {
            // Starting with someone's name picks who acts.
            out.extend(combatants(self.ctx, self.partial).map(|(n, c)| Suggestion {
                line: format!("{n} "),
                label: format!("{n} {}", c.name),
                detail: "acts".into(),
            }));
        }
        out
    }

    /// Combatants, completed to their number.
    fn targets(&self) -> Vec<Suggestion> {
        combatants(self.ctx, self.partial)
            .map(|(n, c)| Suggestion {
                line: self.complete(&n.to_string()),
                label: format!("{n} {}", c.name),
                detail: status(c),
            })
            .collect()
    }

    /// Names that may span several words: offered while what's typed after
    /// the action could still be the start of one.
    fn named(&self, names: Vec<(String, String)>) -> Vec<Suggestion> {
        self.named_after(0, names)
    }

    /// Names, as [`Self::named`], that come `skip` words after the action
    /// (after a count).
    fn named_after(&self, skip: usize, names: Vec<(String, String)>) -> Vec<Suggestion> {
        let mut typed: Vec<&str> = self.args()[skip..].to_vec();
        if !self.partial.is_empty() {
            typed.push(self.partial);
        }
        let typed = typed.join(" ").to_lowercase();
        let head = self.done[..=self.verb_at + skip].join(" ");
        names
            .into_iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&typed))
            .filter(|(name, _)| !(self.partial.is_empty() && name.eq_ignore_ascii_case(&typed)))
            .map(|(name, detail)| Suggestion {
                line: format!("{head} {name} "),
                label: name,
                detail,
            })
            .collect()
    }

    /// First the effect's name, then whom it's on. Once there's a name,
    /// every target is offered, the first included, along with longer names
    /// it could still be the start of, and `for`.
    fn effect(&self) -> Vec<Suggestion> {
        let args = self.args();
        if args.is_empty() {
            return self.named(effect_names(self.ctx));
        }
        // After `for`, only the number of rounds is left.
        if args.iter().any(|w| w.eq_ignore_ascii_case("for")) {
            return Vec::new();
        }
        let mut out = self.targets();
        if !args.iter().any(|w| w.parse::<usize>().is_ok()) {
            out.extend(self.named(effect_names(self.ctx)));
        }
        out.extend(self.keyword(
            "for",
            "for <rounds>",
            "how long it lasts; leave out for until removed",
        ));
        out
    }

    /// The actor's prepared spells (and rituals), then the targets.
    fn cast(&self) -> Vec<Suggestion> {
        let Some(sheet) = self.actor_sheet() else {
            return Vec::new();
        };
        let spells = &sheet.spellcasting.spells;
        match longest_name(self.args(), spells.iter().map(|s| s.name.as_str())) {
            Some(_) if !self.partial.eq_ignore_ascii_case("at") => self.targets(),
            Some(_) => Vec::new(),
            None => self.named(
                spells
                    .iter()
                    .filter(|s| s.prepared || s.ritual)
                    .map(|s| (s.name.clone(), spell_detail(s, sheet)))
                    .collect(),
            ),
        }
    }

    /// The actor's features.
    fn use_feature(&self) -> Vec<Suggestion> {
        let Some(sheet) = self.actor_sheet() else {
            return Vec::new();
        };
        self.named(
            sheet
                .resources
                .iter()
                .map(|r| (r.name.clone(), crate::display::pool(r.uses)))
                .collect(),
        )
    }

    /// Whom to show (on an empty argument), or what.
    fn show(&self) -> Vec<Suggestion> {
        let args = self.args();
        if args.iter().any(|w| w.parse::<usize>().is_ok()) {
            return Vec::new();
        }
        let mut out = if args.is_empty() {
            self.targets()
        } else {
            Vec::new()
        };
        out.extend(self.named(show_names(self.ctx)));
        out
    }

    /// The campaign's characters not yet in the fight (unless counted) and
    /// its monsters; after a whole monster's name, whether to roll its hit
    /// points.
    fn add(&self) -> Vec<Suggestion> {
        let ctx = self.ctx;
        let args = self.args();
        let counted = args.first().is_some_and(|w| w.parse::<u16>().is_ok());
        let skip = usize::from(counted);
        if monster_named(ctx.bestiary, &args[skip..].join(" ")).is_some() {
            return self
                .keyword(
                    "roll",
                    "roll",
                    "roll hit points instead of taking the maximum",
                )
                .into_iter()
                .collect();
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
        self.named_after(skip, names)
    }
}

/// The effects in play, with whom they're on.
fn drop_names(ctx: &Context) -> Vec<(String, String)> {
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
        .collect()
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
    if c.concealed {
        return c.vitality().to_string();
    }
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
