//! The encounter planner's command line: what a line means and what could
//! come next. Changes to the open plan are worked out here, as the plan
//! they'd make, so the screen only has to save it.

use campaign_types::{
    CharacterSummary,
    encounter::{Encounter, EncounterPlan, PlannedMonster},
    monster::MonsterSummary,
};
use uuid::Uuid;

use crate::{
    prompt::Suggestion,
    session::{find_plan, monster_named},
};

pub struct Context<'a> {
    pub encounters: &'a [Encounter],
    /// The plan being worked on.
    pub current: Option<&'a Encounter>,
    pub bestiary: &'a [MonsterSummary],
    pub roster: &'a [CharacterSummary],
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    New(String),
    Open(Uuid),
    /// Save the open plan as this.
    Save(EncounterPlan),
    Delete(Uuid),
    /// Start a fight from the open plan.
    Fight,
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub sentence: String,
    pub plan: Result<Step, String>,
    /// A monster the line is about, to show its stat block.
    pub monster: Option<Uuid>,
}

const VERBS: &[(&str, &str, &str)] = &[
    (
        "add",
        "[count] <monster> [roll]",
        "add monsters to the plan",
    ),
    (
        "remove",
        "<number> [count]",
        "take some or all of an entry out",
    ),
    ("count", "<number> <count>", "set how many of an entry"),
    (
        "party",
        "all | <character>",
        "who it's balanced for: everyone, or toggle one",
    ),
    ("fight", "", "start a fight from this plan"),
    ("new", "<name>", "start a new plan"),
    ("open", "<plan>", "work on another plan"),
    ("rename", "<name>", "rename this plan"),
    ("notes", "<text>", "notes for this plan"),
    ("delete", "<plan>", "delete a plan (type its whole name)"),
    ("quit", "", "leave the planner"),
];

/// How many actions there are: what an empty line lists.
pub const ACTIONS: usize = VERBS.len();

fn verb(word: &str) -> Option<&'static str> {
    let word = word.to_lowercase();
    if let Some((name, ..)) = VERBS.iter().find(|(name, ..)| *name == word) {
        return Some(name);
    }
    let mut matching = VERBS.iter().filter(|(name, ..)| name.starts_with(&word));
    match (matching.next(), matching.next()) {
        (Some((name, ..)), None) => Some(name),
        _ => None,
    }
}

/// The party a plan is balanced for: its chosen characters, or everyone.
pub fn party<'r>(
    plan: &EncounterPlan,
    roster: &'r [CharacterSummary],
) -> Vec<&'r CharacterSummary> {
    roster
        .iter()
        .filter(|c| plan.party.is_empty() || plan.party.contains(&c.id))
        .collect()
}

pub fn parse(line: &str, ctx: &Context) -> Parsed {
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut parsed = Parsed {
        sentence: String::new(),
        plan: Err("type an action; Tab completes".into()),
        monster: None,
    };
    let Some((&first, args)) = words.split_first() else {
        return parsed;
    };
    let Some(verb) = verb(first) else {
        parsed.plan = Err(format!("\"{first}\" isn't an action; Tab lists them"));
        return parsed;
    };
    let text = args.join(" ");
    let open = ctx.current.map(|e| &e.plan);
    let need_open = || Err("open a plan first, or start one with: new <name>".to_string());
    let (sentence, plan) = match verb {
        "new" if text.is_empty() => ("New plan …".into(), Err("what's it called?".into())),
        "new" => match find_plan(ctx.encounters, &text, true) {
            Some(_) => (
                format!("New plan {text}"),
                Err(format!("there's already a plan called {text}; open it")),
            ),
            None => (format!("New plan {text}"), Ok(Step::New(text.clone()))),
        },
        "open" => match find_plan(ctx.encounters, &text, false) {
            Some(e) => (format!("Open {}", e.plan.name), Ok(Step::Open(e.id))),
            None if text.is_empty() => ("Open …".into(), Err("which plan?".into())),
            None => ("Open …".into(), Err(format!("no plan called \"{text}\""))),
        },
        "delete" => match find_plan(ctx.encounters, &text, true) {
            Some(e) => (
                format!("Delete the plan {} (can't be undone)", e.plan.name),
                Ok(Step::Delete(e.id)),
            ),
            None => (
                "Delete …".into(),
                Err("type the whole name of the plan to delete".into()),
            ),
        },
        "quit" => ("Leave the planner".into(), Ok(Step::Quit)),
        _ => match open {
            None => (String::new(), need_open()),
            Some(open) => edit(verb, args, open, ctx, &mut parsed.monster),
        },
    };
    parsed.sentence = sentence;
    parsed.plan = plan;
    parsed
}

/// "4 Goblins": a count and a name.
fn counted(count: u16, name: &str) -> String {
    if count == 1 {
        format!("1 {name}")
    } else {
        format!("{count} × {name}")
    }
}

/// Lines that change the open plan.
fn edit(
    verb: &str,
    args: &[&str],
    open: &EncounterPlan,
    ctx: &Context,
    monster: &mut Option<Uuid>,
) -> (String, Result<Step, String>) {
    let mut plan = open.clone();
    let entry = |word: Option<&&str>| -> Result<usize, String> {
        let word = word.ok_or("which entry? type its number")?;
        word.parse::<usize>()
            .ok()
            .filter(|n| (1..=plan.monsters.len()).contains(n))
            .map(|n| n - 1)
            .ok_or_else(|| format!("there's no entry {word}"))
    };
    match verb {
        "add" => {
            let (count, rest) = match args.split_first() {
                Some((n, rest)) if !rest.is_empty() => match n.parse::<u16>() {
                    Ok(n) => (n, rest),
                    Err(_) => (1, args),
                },
                _ => (1, args),
            };
            let (roll, rest) = match rest.split_last() {
                Some((last, init)) if last.eq_ignore_ascii_case("roll") && !init.is_empty() => {
                    (true, init)
                }
                _ => (false, rest),
            };
            let name = rest.join(" ");
            let Some(found) = monster_named(ctx.bestiary, &name) else {
                let problem = if name.is_empty() {
                    "which monster? Tab lists them".to_string()
                } else {
                    format!("no monster called \"{name}\"; Tab lists them")
                };
                return ("Add …".into(), Err(problem));
            };
            *monster = Some(found.id);
            if count == 0 {
                return ("Add nothing".into(), Err("how many? at least 1".into()));
            }
            let hp = if roll {
                ", rolling their hit points"
            } else {
                ""
            };
            let sentence = format!(
                "Add {} ({} XP){hp}",
                counted(count, &found.name),
                found.xp * u32::from(count)
            );
            match plan
                .monsters
                .iter_mut()
                .find(|m| m.monster == found.id && m.roll == roll)
            {
                Some(entry) => entry.count = entry.count.saturating_add(count),
                None => plan.monsters.push(PlannedMonster {
                    monster: found.id,
                    name: found.name.clone(),
                    count,
                    xp: found.xp,
                    roll,
                }),
            }
            (sentence, Ok(Step::Save(plan)))
        }
        "remove" => {
            let i = match entry(args.first()) {
                Ok(i) => i,
                Err(e) => return ("Remove …".into(), Err(e)),
            };
            *monster = Some(plan.monsters[i].monster);
            let name = plan.monsters[i].name.clone();
            let have = plan.monsters[i].count;
            let fewer = match args.get(1).map(|n| n.parse::<u16>()) {
                None => have,
                Some(Ok(n)) if n > 0 => n.min(have),
                Some(_) => return ("Remove …".into(), Err("how many? at least 1".into())),
            };
            if fewer == have {
                plan.monsters.remove(i);
                (format!("Remove the {name} entry"), Ok(Step::Save(plan)))
            } else {
                plan.monsters[i].count -= fewer;
                (
                    format!("Remove {}", counted(fewer, &name)),
                    Ok(Step::Save(plan)),
                )
            }
        }
        "count" => {
            let i = match entry(args.first()) {
                Ok(i) => i,
                Err(e) => return ("Set the count of …".into(), Err(e)),
            };
            *monster = Some(plan.monsters[i].monster);
            let name = plan.monsters[i].name.clone();
            match args.get(1).map(|n| n.parse::<u16>()) {
                Some(Ok(n)) if n > 0 => {
                    plan.monsters[i].count = n;
                    (
                        format!("{} in the plan", counted(n, &name)),
                        Ok(Step::Save(plan)),
                    )
                }
                _ => (
                    format!("? × {name}"),
                    Err("how many? at least 1 (remove takes it out)".into()),
                ),
            }
        }
        "party" => {
            let text = args.join(" ");
            if text.eq_ignore_ascii_case("all") {
                plan.party.clear();
                return (
                    "Balance it for the whole party".into(),
                    Ok(Step::Save(plan)),
                );
            }
            let lower = text.to_lowercase();
            let matching: Vec<_> = ctx
                .roster
                .iter()
                .filter(|c| c.name.to_lowercase() == lower)
                .chain(
                    ctx.roster
                        .iter()
                        .filter(|c| c.name.to_lowercase().starts_with(&lower)),
                )
                .collect();
            let Some(character) = matching.first().filter(|_| !text.is_empty()) else {
                return (
                    "Party …".into(),
                    Err("all, or a character to add or leave out".into()),
                );
            };
            let mut ids: Vec<Uuid> = party(&plan, ctx.roster).iter().map(|c| c.id).collect();
            let sentence = if ids.contains(&character.id) {
                ids.retain(|id| *id != character.id);
                if ids.is_empty() {
                    let sentence = format!("{} sits this one out", character.name);
                    return (sentence, Err("someone has to be in the party".into()));
                }
                format!("{} sits this one out", character.name)
            } else {
                ids.push(character.id);
                format!("{} is in", character.name)
            };
            // Everyone again means "the whole party", newcomers included.
            plan.party = if ids.len() == ctx.roster.len() {
                Vec::new()
            } else {
                ids
            };
            (sentence, Ok(Step::Save(plan)))
        }
        "rename" => {
            let name = args.join(" ");
            if name.is_empty() {
                return ("Rename to …".into(), Err("what's it called?".into()));
            }
            let taken = ctx.encounters.iter().any(|e| {
                e.plan.name.eq_ignore_ascii_case(&name) && Some(e.id) != ctx.current.map(|c| c.id)
            });
            if taken {
                return (
                    format!("Rename to {name}"),
                    Err(format!("there's already a plan called {name}")),
                );
            }
            plan.name = name.clone();
            (format!("Rename to {name}"), Ok(Step::Save(plan)))
        }
        "notes" => {
            plan.notes = args.join(" ");
            ("Set the notes".into(), Ok(Step::Save(plan)))
        }
        "fight" => {
            if plan.monsters.is_empty() {
                return (
                    "Start a fight".into(),
                    Err("add some monsters first".into()),
                );
            }
            let monsters: Vec<_> = plan
                .monsters
                .iter()
                .map(|m| counted(m.count, &m.name))
                .collect();
            (
                format!("Start a fight: {}", monsters.join(", ")),
                Ok(Step::Fight),
            )
        }
        _ => unreachable!("every verb is handled"),
    }
}

/// What could come next on `line`.
pub fn suggest(line: &str, ctx: &Context) -> Vec<Suggestion> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let typing = !line.is_empty() && !line.ends_with(char::is_whitespace);
    let (done, partial) = match (typing, words.split_last()) {
        (true, Some((last, done))) => (done, *last),
        _ => (&words[..], ""),
    };
    let lower = partial.to_lowercase();
    let Some((&first, args)) = done.split_first() else {
        return VERBS
            .iter()
            .filter(|(name, ..)| name.starts_with(&lower))
            .map(|(name, args, about)| Suggestion {
                line: format!("{name} "),
                label: format!("{name} {args}").trim_end().to_string(),
                detail: about.to_string(),
            })
            .collect();
    };
    let Some(verb) = verb(first) else {
        return Vec::new();
    };
    // Names after `skip` of the arguments, which may span several words.
    let named = |skip: usize, names: Vec<(String, String)>| -> Vec<Suggestion> {
        let mut typed: Vec<&str> = args[skip..].to_vec();
        if !partial.is_empty() {
            typed.push(partial);
        }
        let typed = typed.join(" ").to_lowercase();
        let head = done[..=skip].join(" ");
        names
            .into_iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&typed))
            .filter(|(name, _)| !(partial.is_empty() && name.eq_ignore_ascii_case(&typed)))
            .map(|(name, detail)| Suggestion {
                line: format!("{head} {name} "),
                label: name,
                detail,
            })
            .collect()
    };
    let entries = || -> Vec<Suggestion> {
        let Some(open) = ctx.current else {
            return Vec::new();
        };
        open.plan
            .monsters
            .iter()
            .enumerate()
            .map(|(i, m)| (i + 1, m))
            .filter(|(n, _)| n.to_string().starts_with(&lower))
            .map(|(n, m)| Suggestion {
                line: format!("{first} {n} "),
                label: format!("{n} {}", m.name),
                detail: format!("{} of them", m.count),
            })
            .collect()
    };
    let plans = || {
        ctx.encounters
            .iter()
            .map(|e| {
                (
                    e.plan.name.clone(),
                    format!("{} monsters", e.plan.monster_count()),
                )
            })
            .collect()
    };
    match verb {
        "add" => {
            let counted = args.first().is_some_and(|w| w.parse::<u16>().is_ok());
            let skip = usize::from(counted);
            if monster_named(ctx.bestiary, &args[skip..].join(" ")).is_some() {
                return if "roll".starts_with(&lower) {
                    vec![Suggestion {
                        line: format!("{} roll ", done.join(" ")),
                        label: "roll".into(),
                        detail: "roll their hit points when the fight starts".into(),
                    }]
                } else {
                    Vec::new()
                };
            }
            named(
                skip,
                ctx.bestiary
                    .iter()
                    .map(|m| {
                        let whose = if m.shared { "" } else { " · campaign's own" };
                        (
                            m.name.clone(),
                            format!(
                                "CR {} · {} XP · {} {}{whose}",
                                m.challenge, m.xp, m.size, m.kind
                            ),
                        )
                    })
                    .collect(),
            )
        }
        "remove" | "count" if args.is_empty() => entries(),
        "open" | "delete" => named(0, plans()),
        "party" => {
            let mut names = vec![("all".to_string(), "the whole party".to_string())];
            if let Some(open) = ctx.current {
                let in_party: Vec<Uuid> =
                    party(&open.plan, ctx.roster).iter().map(|c| c.id).collect();
                names.extend(ctx.roster.iter().map(|c| {
                    let state = if in_party.contains(&c.id) {
                        "in"
                    } else {
                        "out"
                    };
                    (c.name.clone(), format!("level {} · {state}", c.level))
                }));
            }
            named(0, names)
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    struct World {
        encounters: Vec<Encounter>,
        bestiary: Vec<MonsterSummary>,
        roster: Vec<CharacterSummary>,
    }

    fn monster(name: &str, challenge: &str, xp: u32) -> MonsterSummary {
        MonsterSummary {
            id: Uuid::new_v4(),
            name: name.into(),
            size: "Small".into(),
            kind: "fey".into(),
            challenge: challenge.into(),
            hit_points: 7,
            max_hit_points: 12,
            xp,
            armor_class: 15,
            source: "SRD 5.2".into(),
            shared: true,
        }
    }

    fn character(name: &str, level: u8) -> CharacterSummary {
        CharacterSummary {
            id: Uuid::new_v4(),
            name: name.into(),
            player_name: None,
            race: "Gnome".into(),
            level,
            classes: Vec::new(),
            updated_at: Utc::now(),
        }
    }

    fn encounter(plan: EncounterPlan) -> Encounter {
        Encounter {
            id: Uuid::new_v4(),
            campaign: Uuid::new_v4(),
            plan,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    /// A "Goblin patrol" with 3 goblins, and an empty "Troll bridge".
    fn world() -> World {
        let bestiary = vec![
            monster("Goblin", "1/4", 50),
            monster("Goblin Boss", "1", 200),
            monster("Troll", "5", 1800),
        ];
        let patrol = EncounterPlan {
            name: "Goblin patrol".into(),
            monsters: vec![PlannedMonster {
                monster: bestiary[0].id,
                name: "Goblin".into(),
                count: 3,
                xp: 50,
                roll: false,
            }],
            ..Default::default()
        };
        let bridge = EncounterPlan {
            name: "Troll bridge".into(),
            ..Default::default()
        };
        World {
            encounters: vec![encounter(patrol), encounter(bridge)],
            bestiary,
            roster: vec![character("Vex", 5), character("Pike", 5)],
        }
    }

    impl World {
        fn ctx(&self, open: usize) -> Context<'_> {
            Context {
                encounters: &self.encounters,
                current: self.encounters.get(open),
                bestiary: &self.bestiary,
                roster: &self.roster,
            }
        }

        fn saved(&self, line: &str) -> EncounterPlan {
            match parse(line, &self.ctx(0)).plan {
                Ok(Step::Save(plan)) => plan,
                other => panic!("{line}: {other:?}"),
            }
        }

        fn problem(&self, line: &str) -> String {
            parse(line, &self.ctx(0)).plan.unwrap_err()
        }

        fn lines(&self, line: &str) -> Vec<String> {
            suggest(line, &self.ctx(0))
                .into_iter()
                .map(|s| s.line)
                .collect()
        }
    }

    #[test]
    fn adding_counts_and_merges_with_an_entry_of_the_same_monster() {
        let w = world();
        let p = parse("add 2 goblin boss", &w.ctx(0));
        assert_eq!(p.sentence, "Add 2 × Goblin Boss (400 XP)");
        assert_eq!(p.monster, Some(w.bestiary[1].id), "its stat block shows");
        let plan = w.saved("add 2 goblin boss");
        assert_eq!(plan.monsters.len(), 2);
        assert_eq!((plan.monsters[1].count, plan.monsters[1].xp), (2, 200));

        let plan = w.saved("add 2 goblin");
        assert_eq!(plan.monsters.len(), 1, "more goblins join the goblins");
        assert_eq!(plan.monsters[0].count, 5);
        let plan = w.saved("add goblin roll");
        assert_eq!(plan.monsters.len(), 2, "rolled ones are their own entry");
        assert!(plan.monsters[1].roll);

        assert_eq!(
            w.problem("add 2 dragon"),
            "no monster called \"dragon\"; Tab lists them"
        );
        assert_eq!(w.problem("add 0 goblin"), "how many? at least 1");
    }

    #[test]
    fn entries_are_changed_by_number() {
        let w = world();
        assert_eq!(w.saved("count 1 7").monsters[0].count, 7);
        assert_eq!(w.saved("remove 1 2").monsters[0].count, 1);
        assert!(w.saved("remove 1").monsters.is_empty());
        assert!(
            w.saved("remove 1 9").monsters.is_empty(),
            "no fewer than none"
        );
        assert_eq!(w.problem("remove 2"), "there's no entry 2");
        assert_eq!(
            w.problem("count 1 0"),
            "how many? at least 1 (remove takes it out)"
        );
    }

    #[test]
    fn the_party_is_everyone_unless_someone_sits_out() {
        let w = world();
        let pike = w.roster[1].id;
        let plan = w.saved("party vex");
        assert_eq!(plan.party, [pike], "Vex sits it out");
        let mut w2 = world();
        w2.encounters[0].plan.party = vec![w2.roster[1].id];
        let back = match parse("party vex", &w2.ctx(0)).plan {
            Ok(Step::Save(plan)) => plan,
            other => panic!("{other:?}"),
        };
        assert!(back.party.is_empty(), "everyone again is the whole party");
        assert_eq!(
            parse("party pike", &w2.ctx(0)).plan,
            Err("someone has to be in the party".into())
        );
        assert!(w.saved("party all").party.is_empty());
    }

    #[test]
    fn plans_are_made_opened_renamed_and_deleted_by_name() {
        let w = world();
        let bridge = w.encounters[1].id;
        assert_eq!(
            parse("new Ambush", &w.ctx(0)).plan,
            Ok(Step::New("Ambush".into()))
        );
        assert!(parse("new goblin patrol", &w.ctx(0)).plan.is_err(), "taken");
        assert_eq!(parse("open tr", &w.ctx(0)).plan, Ok(Step::Open(bridge)));
        assert_eq!(
            parse("o tr", &w.ctx(0)).plan,
            Ok(Step::Open(bridge)),
            "verbs shorten"
        );
        assert!(
            parse("delete troll", &w.ctx(0)).plan.is_err(),
            "the whole name"
        );
        assert_eq!(
            parse("delete troll bridge", &w.ctx(0)).plan,
            Ok(Step::Delete(bridge))
        );
        assert_eq!(w.saved("rename Patrol").name, "Patrol");
        assert_eq!(
            w.problem("rename troll bridge"),
            "there's already a plan called troll bridge"
        );
        let none = Context {
            current: None,
            ..w.ctx(0)
        };
        assert_eq!(
            parse("add goblin", &none).plan,
            Err("open a plan first, or start one with: new <name>".into())
        );
    }

    #[test]
    fn a_fight_needs_monsters() {
        let w = world();
        let p = parse("fight", &w.ctx(0));
        assert_eq!(
            (p.sentence.as_str(), p.plan),
            ("Start a fight: 3 × Goblin", Ok(Step::Fight))
        );
        assert_eq!(
            parse("fight", &w.ctx(1)).plan,
            Err("add some monsters first".into())
        );
    }

    #[test]
    fn completion_offers_monsters_entries_plans_and_the_party() {
        let w = world();
        assert_eq!(w.lines("ad"), ["add "]);
        assert_eq!(
            w.lines("add 2 gob"),
            ["add 2 Goblin ", "add 2 Goblin Boss "]
        );
        assert_eq!(w.lines("add 2 goblin "), ["add 2 goblin roll "]);
        assert_eq!(w.lines("remove "), ["remove 1 "]);
        assert_eq!(w.lines("open t"), ["open Troll bridge "]);
        assert_eq!(
            w.lines("party "),
            ["party all ", "party Vex ", "party Pike "]
        );
        let details: Vec<_> = suggest("add tro", &w.ctx(0))
            .into_iter()
            .map(|s| s.detail)
            .collect();
        assert_eq!(details, ["CR 5 · 1800 XP · Small fey"]);
    }
}
