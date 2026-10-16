//! Human-readable rendering of characters.

use std::fmt::Write;

use campaign_types::{
    Ability, Character, CharacterSheet, CharacterSummary, Pool, Recharge, Skill,
    monster::{Feature, MonsterSheet, MonsterSummary},
};

use crate::parse::short_id;

pub fn list(characters: &[CharacterSummary]) -> String {
    if characters.is_empty() {
        return "No characters yet. Create one with `campaign-edit new <NAME>`.".into();
    }
    let width = characters.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for c in characters {
        let classes = classes_line(&c.classes);
        let player = c
            .player_name
            .as_deref()
            .map(|p| format!("  ({p})"))
            .unwrap_or_default();
        let line = format!(
            "{}  {:<width$}  {}{}",
            short_id(c.id),
            c.name,
            [c.race.as_str(), classes.as_str()].join(" ").trim(),
            player
        );
        let _ = writeln!(out, "{}", line.trim_end());
    }
    out.trim_end().to_string()
}

pub fn sheet(character: &Character) -> String {
    let s = &character.sheet;
    let mut out = String::new();

    let identity = [s.race.as_str(), &classes_line(&s.classes)].join(" ");
    let _ = writeln!(out, "{}  {}", s.name, identity.trim());
    let mut details = vec![format!("ID {}", short_id(character.id))];
    if let Some(player) = &s.player_name {
        details.push(format!("Player {player}"));
    }
    if !s.background.is_empty() {
        details.push(format!("Background {}", s.background));
    }
    if !s.alignment.is_empty() {
        details.push(format!("Alignment {}", s.alignment));
    }
    details.push(format!("XP {}", s.experience_points));
    details.push(format!("v{}", character.version));
    let _ = writeln!(out, "{}\n", details.join("  ·  "));

    let hp = s.hit_points;
    let temp = if hp.temporary > 0 {
        format!(" (+{} temp)", hp.temporary)
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "HP {}/{}{temp}   AC {}   Speed {} ft   Proficiency {:+}   Passive Perception {}\n",
        hp.current,
        hp.max,
        s.armor_class,
        s.speed,
        s.proficiency_bonus(),
        10 + s.skill_bonus(Skill::Perception),
    );

    let abilities: Vec<_> = Ability::ALL
        .iter()
        .map(|&a| {
            format!(
                "{} {:>2} ({:+})",
                abbreviation(a),
                s.ability_scores.get(a),
                s.ability_scores.modifier(a)
            )
        })
        .collect();
    let _ = writeln!(out, "{}", abilities.join("   "));
    let saves: Vec<_> = Ability::ALL
        .iter()
        .map(|&a| {
            let mark = if s.saving_throw_proficiencies.contains(&a) {
                "*"
            } else {
                ""
            };
            format!("{} {:+}{mark}", abbreviation(a), s.saving_throw(a))
        })
        .collect();
    let _ = writeln!(out, "Saves  {}", saves.join("  "));
    if !s.skill_proficiencies.is_empty() {
        let skills: Vec<_> = s
            .skill_proficiencies
            .iter()
            .map(|&k| format!("{} {:+}", skill_name(k), s.skill_bonus(k)))
            .collect();
        let _ = writeln!(out, "Skills {}", skills.join(", "));
    }

    spellcasting(&mut out, s);

    if !s.resources.is_empty() {
        let _ = writeln!(out, "\nFeatures");
        let width = s.resources.iter().map(|r| r.name.len()).max().unwrap_or(0);
        for r in &s.resources {
            let _ = writeln!(
                out,
                "  {:<width$}  {}  {}",
                r.name,
                pool(r.uses),
                recharge(r.recharge)
            );
        }
    }

    if !s.inventory.is_empty() {
        let _ = writeln!(out, "\nInventory");
        for item in &s.inventory {
            let qty = if item.quantity == 1 {
                String::new()
            } else {
                format!(" ×{}", item.quantity)
            };
            let notes = if item.notes.is_empty() {
                String::new()
            } else {
                format!(" — {}", item.notes)
            };
            let _ = writeln!(out, "  {}{qty}{notes}", item.name);
        }
    }

    if !s.notes.is_empty() {
        let _ = writeln!(out, "\nNotes");
        for line in s.notes.lines() {
            let _ = writeln!(out, "  {line}");
        }
    }
    out.trim_end().to_string()
}

fn spellcasting(out: &mut String, s: &CharacterSheet) {
    let casting = &s.spellcasting;
    let casters: Vec<_> = s
        .classes
        .iter()
        .filter_map(|c| c.spellcasting_ability.map(|a| (c, a)))
        .collect();
    if casting.spells.is_empty()
        && casting.slots.is_empty()
        && casting.pact_magic.is_none()
        && casters.is_empty()
    {
        return;
    }
    let _ = writeln!(out, "\nSpellcasting");
    for (class, ability) in casters {
        let _ = writeln!(
            out,
            "  {} ({}): save DC {}, attack {:+}",
            class.name,
            abbreviation(ability),
            s.spell_save_dc(ability),
            s.spell_attack_bonus(ability)
        );
    }
    if let Some(slots) = slots_line(s) {
        let _ = writeln!(out, "  Slots  {slots}");
    }
    for level in 0..=9 {
        let spells: Vec<_> = casting
            .spells
            .iter()
            .filter(|sp| sp.level == level)
            .map(|sp| {
                let mut tags = Vec::new();
                if sp.concentration {
                    tags.push("C");
                }
                if sp.ritual {
                    tags.push("R");
                }
                if !sp.prepared {
                    tags.push("unprepared");
                }
                if tags.is_empty() {
                    sp.name.clone()
                } else {
                    format!("{} ({})", sp.name, tags.join(", "))
                }
            })
            .collect();
        if !spells.is_empty() {
            let label = if level == 0 {
                "Cantrips".to_string()
            } else {
                ordinal(level)
            };
            let _ = writeln!(out, "  {label:<8} {}", spells.join(", "));
        }
    }
}

/// The in-play state that changes during a session: HP, slots, features.
pub fn status(s: &CharacterSheet) -> String {
    let hp = s.hit_points;
    let mut lines = vec![format!("HP {}/{}", hp.current, hp.max)];
    if let Some(slots) = slots_line(s) {
        lines.push(format!("Slots  {slots}"));
    }
    lines.extend(
        s.resources
            .iter()
            .map(|r| format!("{}  {}", r.name, pool(r.uses))),
    );
    lines.join("\n")
}

pub fn slots_line(s: &CharacterSheet) -> Option<String> {
    let casting = &s.spellcasting;
    let mut slots: Vec<_> = casting
        .slots
        .iter()
        .map(|(level, p)| format!("{} {}", ordinal(*level), pool(*p)))
        .collect();
    if let Some(pact) = casting.pact_magic {
        slots.push(format!(
            "Pact ({}) {}",
            ordinal(pact.slot_level),
            pool(pact.slots)
        ));
    }
    (!slots.is_empty()).then(|| slots.join("   "))
}

/// `●●○ 2/3` — filled pips are uses remaining.
pub fn pool(p: Pool) -> String {
    let remaining = p.remaining() as usize;
    let spent = p.used.min(p.max) as usize;
    if p.max <= 10 {
        format!(
            "{}{} {}/{}",
            "●".repeat(remaining),
            "○".repeat(spent),
            remaining,
            p.max
        )
    } else {
        format!("{}/{}", remaining, p.max)
    }
}

fn recharge(r: Recharge) -> &'static str {
    match r {
        Recharge::ShortRest => "short rest",
        Recharge::LongRest => "long rest",
        Recharge::Manual => "manual",
    }
}

pub fn ordinal(level: u8) -> String {
    let suffix = match level {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    format!("{level}{suffix}")
}

fn classes_line(classes: &[campaign_types::ClassLevel]) -> String {
    classes
        .iter()
        .map(|c| match &c.subclass {
            Some(sub) => format!("{} {} ({sub})", c.name, c.level),
            None => format!("{} {}", c.name, c.level),
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Monsters as a table: name, CR, size and type, AC, HP, where from.
pub fn monster_list(monsters: &[MonsterSummary]) -> String {
    if monsters.is_empty() {
        return "No monsters.".into();
    }
    let width = monsters
        .iter()
        .map(|m| m.name.chars().count())
        .max()
        .unwrap_or(0);
    let kind = monsters
        .iter()
        .map(|m| m.size.len() + 1 + m.kind.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for m in monsters {
        let what = format!("{} {}", m.size, m.kind);
        let whose = if m.shared { "" } else { "  (campaign)" };
        let _ = writeln!(
            out,
            "{:<width$}  CR {:<4}  {what:<kind$}  AC {:>2}  HP {:>3}  {}{whose}",
            m.name, m.challenge, m.armor_class, m.hit_points, m.source
        );
    }
    out.trim_end().to_string()
}

/// A stat block, laid out like the books'.
pub fn monster(m: &MonsterSheet) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", m.name);
    let mut kind = format!("{} {}", m.size, m.kind);
    if !m.alignment.is_empty() {
        kind += &format!(", {}", m.alignment);
    }
    let _ = writeln!(out, "{kind}  ·  {}\n", m.source);

    let armor = if m.armor.is_empty() {
        String::new()
    } else {
        format!(" ({})", m.armor)
    };
    let dice = match m.hit_dice() {
        Some(_) => format!(" ({}, max {})", m.hit_dice, m.max_hit_points()),
        None => String::new(),
    };
    let _ = writeln!(
        out,
        "AC {}{armor}   HP {}{dice}   Speed {}   Initiative {:+}",
        m.armor_class,
        m.hit_points,
        m.speed,
        m.initiative_modifier()
    );
    let abilities: Vec<_> = Ability::ALL
        .iter()
        .map(|&a| {
            let score = m.ability_scores.get(a);
            format!(
                "{} {score} ({:+})",
                abbreviation(a),
                m.ability_scores.modifier(a)
            )
        })
        .collect();
    let _ = writeln!(out, "{}", abilities.join("  "));

    let mut details = Vec::new();
    if !m.saving_throws.is_empty() {
        let saves: Vec<_> = m
            .saving_throws
            .iter()
            .map(|(a, b)| format!("{} {b:+}", abbreviation(*a)))
            .collect();
        details.push(format!("Saves {}", saves.join(", ")));
    }
    if !m.skills.is_empty() {
        let skills: Vec<_> = m
            .skills
            .iter()
            .map(|(s, b)| format!("{} {b:+}", skill_name(*s)))
            .collect();
        details.push(format!("Skills {}", skills.join(", ")));
    }
    for (label, list) in [
        ("Vulnerabilities", &m.damage_vulnerabilities),
        ("Resistances", &m.damage_resistances),
        ("Immunities", &m.damage_immunities),
        ("Condition immunities", &m.condition_immunities),
    ] {
        if !list.is_empty() {
            details.push(format!("{label} {}", list.join(", ")));
        }
    }
    if !m.senses.is_empty() {
        details.push(format!("Senses {}", m.senses));
    }
    if !m.languages.is_empty() {
        details.push(format!("Languages {}", m.languages));
    }
    details.push(format!(
        "CR {} ({} XP)   Proficiency {:+}",
        m.challenge, m.xp, m.proficiency_bonus
    ));
    let _ = writeln!(out, "{}", details.join("\n"));

    for (heading, features) in [
        ("Traits", &m.traits),
        ("Actions", &m.actions),
        ("Bonus actions", &m.bonus_actions),
        ("Reactions", &m.reactions),
        ("Legendary actions", &m.legendary_actions),
    ] {
        if !features.is_empty() {
            let _ = writeln!(out, "\n{heading}");
            for Feature { name, description } in features {
                let _ = writeln!(out, "  {name}. {description}");
            }
        }
    }
    if !m.notes.is_empty() {
        let _ = writeln!(out, "\nNotes\n  {}", m.notes);
    }
    out.trim_end().to_string()
}

fn abbreviation(a: Ability) -> &'static str {
    match a {
        Ability::Strength => "STR",
        Ability::Dexterity => "DEX",
        Ability::Constitution => "CON",
        Ability::Intelligence => "INT",
        Ability::Wisdom => "WIS",
        Ability::Charisma => "CHA",
    }
}

fn skill_name(skill: Skill) -> String {
    // AnimalHandling -> Animal Handling
    let debug = format!("{skill:?}");
    let mut name = String::new();
    for (i, c) in debug.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            name.push(' ');
        }
        name.push(c);
    }
    name.replace(" Of ", " of ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_render_as_pips() {
        assert_eq!(pool(Pool { max: 3, used: 1 }), "●●○ 2/3");
        assert_eq!(pool(Pool { max: 20, used: 5 }), "15/20");
    }

    #[test]
    fn names_render_nicely() {
        assert_eq!(skill_name(Skill::SleightOfHand), "Sleight of Hand");
        assert_eq!(ordinal(2), "2nd");
    }
}
