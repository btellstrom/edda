//! Human-readable rendering of characters and monsters.

use std::fmt::Write;

use ratatui::text::{Line, Span};

use edda_types::{
    Ability, Character, CharacterSheet, CharacterSummary, Edition, Pool, Recharge, Skill,
    monster::{Feature, MonsterSheet, MonsterSummary},
    rules_text::{ConditionText, Entry, RulesText, SpellText},
};

use crate::{
    parse::short_id,
    styled::{Doc, KEEP, dim, plain, strong, together},
};

pub fn list(characters: &[CharacterSummary]) -> String {
    if characters.is_empty() {
        return "No characters yet. Create one with `edda new <NAME>`.".into();
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

/// `label value` pairs, separated by `sep`.
fn pairs(items: Vec<(String, Vec<Span<'static>>)>, sep: &Span<'static>) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (label, value)) in items.into_iter().enumerate() {
        if i > 0 {
            spans.push(sep.clone());
        }
        // A label stays on the same line as its value.
        spans.push(dim(format!("{}{KEEP}", together(&label))));
        spans.extend(
            value
                .into_iter()
                .map(|v| Span::styled(together(&v.content), v.style)),
        );
    }
    spans
}

/// A score and its modifier: `18 (+4)`.
fn score(score: u8, modifier: i8) -> Vec<Span<'static>> {
    vec![
        plain(format!("{score:>2}")),
        dim(format!(" ({modifier:+})")),
    ]
}

/// The headed parts of a character sheet or stat block, which the fight
/// screen can scroll its panel to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Features,
    Inventory,
    Notes,
    Spellcasting,
    Traits,
    Actions,
    BonusActions,
    Reactions,
    LegendaryActions,
}

impl Section {
    /// Its heading, as drawn.
    pub fn title(self) -> &'static str {
        match self {
            Section::Features => "Features",
            Section::Inventory => "Inventory",
            Section::Notes => "Notes",
            Section::Spellcasting => "Spellcasting",
            Section::Traits => "Traits",
            Section::Actions => "Actions",
            Section::BonusActions => "Bonus actions",
            Section::Reactions => "Reactions",
            Section::LegendaryActions => "Legendary actions",
        }
    }
}

/// How much of a sheet to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// Everything, for `show`.
    Full,
    /// What matters in a fight, in a panel whose title names whose it is:
    /// no name, identity or bookkeeping, and the abilities as a table.
    Panel,
}

/// A character sheet, `width` columns wide.
pub fn sheet(character: &Character, width: usize, detail: Detail) -> Vec<Line<'static>> {
    let s = &character.sheet;
    let mut doc = Doc::new(width);
    if detail == Detail::Full {
        identity(&mut doc, character);
    }
    hit_points_line(&mut doc, s);
    doc.blank();
    match detail {
        Detail::Full => abilities_and_saves(&mut doc, s),
        Detail::Panel => ability_table(&mut doc, s),
    }
    sections(&mut doc, s);
    doc.into_lines()
}

/// The name, race and classes, and who plays them.
fn identity(doc: &mut Doc, character: &Character) {
    let s = &character.sheet;
    let identity = [s.race.as_str(), &classes_line(&s.classes)].join(" ");
    doc.line(vec![
        strong(s.name.clone()),
        plain("  "),
        plain(identity.trim().to_string()),
    ]);
    let mut details = vec![("ID".to_string(), vec![plain(short_id(character.id))])];
    if let Some(player) = &s.player_name {
        details.push(("Player".into(), vec![plain(player.clone())]));
    }
    if !s.background.is_empty() {
        details.push(("Background".into(), vec![plain(s.background.clone())]));
    }
    if !s.alignment.is_empty() {
        details.push(("Alignment".into(), vec![plain(s.alignment.clone())]));
    }
    details.push(("XP".into(), vec![plain(s.experience_points.to_string())]));
    // The dots stay with what they follow, so a wrapped line never starts
    // with one.
    let mut line = pairs(details, &dim(format!("{KEEP}{KEEP}·  ")));
    line.push(dim(format!("{KEEP}{KEEP}·  v{}", character.version)));
    doc.line(line);
    doc.blank();
}

fn hit_points_line(doc: &mut Doc, s: &CharacterSheet) {
    let hp = s.hit_points;
    let mut hit_points = vec![strong(format!("{}/{}", hp.current, hp.max))];
    if hp.temporary > 0 {
        hit_points.push(dim(format!(" (+{} temp)", hp.temporary)));
    }
    doc.line(pairs(
        vec![
            ("HP".into(), hit_points),
            ("AC".into(), vec![strong(s.armor_class.to_string())]),
            ("Speed".into(), vec![plain(format!("{} ft", s.speed))]),
            (
                "Prof".into(),
                vec![plain(format!("{:+}", s.proficiency_bonus()))],
            ),
            (
                "Passive Perception".into(),
                vec![plain((10 + s.skill_bonus(Skill::Perception)).to_string())],
            ),
        ],
        &plain("   "),
    ));
}

/// The abilities' modifiers and saves as a table, a column each.
fn ability_table(doc: &mut Doc, s: &CharacterSheet) {
    let mut header = vec![plain("    ")];
    let mut modifiers = vec![dim("Mod ")];
    let mut saves = vec![dim("Save")];
    for &a in &Ability::ALL {
        header.push(dim(format!("  {:>4} ", abbreviation(a))));
        modifiers.push(plain(format!(
            "  {:>4} ",
            format!("{:+}", s.ability_scores.modifier(a))
        )));
        let bonus = format!("  {:>4}", format!("{:+}", s.saving_throw(a)));
        // Proficient saves stand out, and keep their mark for plain text.
        saves.push(if s.saving_throw_proficiencies.contains(&a) {
            strong(format!("{bonus}*"))
        } else {
            plain(format!("{bonus} "))
        });
    }
    for row in [header, modifiers, saves] {
        doc.line(row);
    }
}

fn abilities_and_saves(doc: &mut Doc, s: &CharacterSheet) {
    doc.line(pairs(
        Ability::ALL
            .iter()
            .map(|&a| {
                (
                    abbreviation(a).to_string(),
                    score(s.ability_scores.get(a), s.ability_scores.modifier(a)),
                )
            })
            .collect(),
        &plain("   "),
    ));
    let mut saves = Vec::new();
    for (i, &a) in Ability::ALL.iter().enumerate() {
        if i > 0 {
            saves.push(plain("  "));
        }
        saves.push(dim(format!("{} ", abbreviation(a))));
        let bonus = format!("{:+}", s.saving_throw(a));
        // Proficient saves stand out, and keep their mark for plain text.
        if s.saving_throw_proficiencies.contains(&a) {
            saves.push(strong(format!("{bonus}*")));
        } else {
            saves.push(plain(bonus));
        }
    }
    doc.row(0, "Saves", 6, saves);
}

/// Skills, then spells, features, inventory and notes.
fn sections(doc: &mut Doc, s: &CharacterSheet) {
    if !s.skill_proficiencies.is_empty() {
        let mut skills = Vec::new();
        for (i, &k) in s.skill_proficiencies.iter().enumerate() {
            if i > 0 {
                skills.push(plain(", "));
            }
            skills.push(plain(together(&format!(
                "{} {:+}",
                skill_name(k),
                s.skill_bonus(k)
            ))));
        }
        doc.row(0, "Skills", 6, skills);
    }

    spellcasting(doc, s);

    if !s.resources.is_empty() {
        doc.heading(Section::Features.title());
        let name_width = s
            .resources
            .iter()
            .map(|r| chars(&r.name))
            .max()
            .unwrap_or(0);
        for r in &s.resources {
            doc.bullet(vec![
                plain(format!("{:<name_width$}  ", r.name)),
                plain(pool(r.uses)),
                dim(format!("  {}", recharge(r.recharge))),
            ]);
        }
    }

    if !s.inventory.is_empty() {
        doc.heading(Section::Inventory.title());
        for item in &s.inventory {
            let mut entry = vec![plain(item.name.clone())];
            if item.quantity != 1 {
                entry.push(plain(format!(" ×{}", item.quantity)));
            }
            if !item.notes.is_empty() {
                entry.push(dim(" — "));
                entry.push(plain(item.notes.clone()));
            }
            doc.bullet(entry);
        }
    }

    if !s.notes.is_empty() {
        doc.heading(Section::Notes.title());
        notes(doc, &s.notes);
    }
}

/// `text` with dice expressions ("1d10 + 8") kept on one line.
fn dice_together(text: &str) -> String {
    let is_dice = |word: &str| {
        let word = word.trim_start_matches('(');
        word.split_once('d').is_some_and(|(count, sides)| {
            count.chars().all(|c| c.is_ascii_digit())
                && !sides.is_empty()
                && sides.chars().all(|c| c.is_ascii_digit())
        })
    };
    let words: Vec<&str> = text.split(' ').collect();
    let mut out = String::new();
    for (i, word) in words.iter().enumerate() {
        if i > 0 {
            let operator = |w: &str| matches!(w, "+" | "-" | "−");
            let joined = (operator(word) && is_dice(words[i - 1]))
                || (operator(words[i - 1]) && i >= 2 && is_dice(words[i - 2]));
            out.push(if joined { KEEP } else { ' ' });
        }
        out.push_str(word);
    }
    out
}

/// Free text, paragraph by paragraph, keeping its blank lines.
fn notes(doc: &mut Doc, text: &str) {
    for line in text.trim_end().lines() {
        if line.trim().is_empty() {
            doc.blank();
        } else {
            doc.paragraph(vec![plain(line.to_string())]);
        }
    }
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn spellcasting(doc: &mut Doc, s: &CharacterSheet) {
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
    doc.heading(Section::Spellcasting.title());

    let mut rows: Vec<(String, Vec<Span<'static>>)> = Vec::new();
    for (class, ability) in casters {
        rows.push((
            format!("{} ({})", class.name, abbreviation(ability)),
            pairs(
                vec![
                    (
                        "save DC".into(),
                        vec![strong(s.spell_save_dc(ability).to_string())],
                    ),
                    (
                        "attack".into(),
                        vec![strong(format!("{:+}", s.spell_attack_bonus(ability)))],
                    ),
                ],
                &plain("   "),
            ),
        ));
    }
    if let Some(slots) = slots_line(s) {
        // Each level's pool stays on one line.
        let mut value = Vec::new();
        for (i, level) in slots.split("   ").enumerate() {
            if i > 0 {
                value.push(plain("   "));
            }
            value.push(plain(together(level)));
        }
        rows.push(("Slots".into(), value));
    }
    for level in 0..=9 {
        let mut spells = Vec::new();
        for spell in casting.spells.iter().filter(|sp| sp.level == level) {
            if !spells.is_empty() {
                spells.push(plain(", "));
            }
            spells.push(plain(spell.name.clone()));
            let mut tags = Vec::new();
            if spell.concentration {
                tags.push("C");
            }
            if spell.ritual {
                tags.push("R");
            }
            if !spell.prepared {
                tags.push("unprepared");
            }
            if !tags.is_empty() {
                spells.push(dim(together(&format!(" ({})", tags.join(", ")))));
            }
        }
        if !spells.is_empty() {
            let label = if level == 0 {
                "Cantrips".to_string()
            } else {
                ordinal(level)
            };
            rows.push((label, spells));
        }
    }
    let width = rows
        .iter()
        .map(|(label, _)| chars(label))
        .max()
        .unwrap_or(0);
    for (label, value) in rows {
        doc.row(2, &label, width, value);
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

fn classes_line(classes: &[edda_types::ClassLevel]) -> String {
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

/// A stat block, laid out like the books', `width` columns wide.
pub fn monster(m: &MonsterSheet, width: usize, detail: Detail) -> Vec<Line<'static>> {
    let mut doc = Doc::new(width);
    if detail == Detail::Full {
        doc.line(vec![strong(m.name.clone())]);
    }
    let mut kind = format!("{} {}", m.size, m.kind);
    if !m.alignment.is_empty() {
        kind += &format!(", {}", m.alignment);
    }
    doc.line(vec![plain(kind), dim(format!("  ·  {}", m.source))]);
    doc.blank();

    let mut armor = vec![strong(m.armor_class.to_string())];
    if !m.armor.is_empty() {
        armor.push(dim(format!(" ({})", m.armor)));
    }
    let mut hit_points = vec![strong(m.hit_points.to_string())];
    if m.hit_dice().is_some() {
        hit_points.push(dim(format!(
            " ({}, max {})",
            m.hit_dice,
            m.max_hit_points()
        )));
    }
    doc.line(pairs(
        vec![
            ("AC".into(), armor),
            ("HP".into(), hit_points),
            ("Speed".into(), vec![plain(m.speed.clone())]),
            (
                "Initiative".into(),
                vec![plain(format!("{:+}", m.initiative_modifier()))],
            ),
        ],
        &plain("   "),
    ));
    doc.line(pairs(
        Ability::ALL
            .iter()
            .map(|&a| {
                (
                    abbreviation(a).to_string(),
                    score(m.ability_scores.get(a), m.ability_scores.modifier(a)),
                )
            })
            .collect(),
        &plain("  "),
    ));
    doc.blank();

    for (label, value) in details(m) {
        doc.line(vec![dim(format!("{label} ")), plain(value)]);
    }
    doc.line(pairs(
        vec![
            (
                "CR".into(),
                vec![
                    strong(m.challenge.clone()),
                    dim(format!(" ({} XP)", m.experience_points())),
                ],
            ),
            (
                "Proficiency".into(),
                vec![plain(format!("{:+}", m.proficiency_bonus))],
            ),
        ],
        &plain("   "),
    ));

    features(&mut doc, m);
    if !m.notes.is_empty() {
        doc.heading(Section::Notes.title());
        notes(&mut doc, &m.notes);
    }
    doc.into_lines()
}

/// A stat block's lesser details: saves, skills, damage and condition
/// immunities, senses and languages, those it has.
fn details(m: &MonsterSheet) -> Vec<(&'static str, String)> {
    let mut details: Vec<(&'static str, String)> = Vec::new();
    if !m.saving_throws.is_empty() {
        let saves: Vec<_> = m
            .saving_throws
            .iter()
            .map(|(a, b)| format!("{} {b:+}", abbreviation(*a)))
            .collect();
        details.push(("Saves", saves.join(", ")));
    }
    if !m.skills.is_empty() {
        let skills: Vec<_> = m
            .skills
            .iter()
            .map(|(s, b)| format!("{} {b:+}", skill_name(*s)))
            .collect();
        details.push(("Skills", skills.join(", ")));
    }
    for (label, list) in [
        ("Vulnerabilities", &m.damage_vulnerabilities),
        ("Resistances", &m.damage_resistances),
        ("Immunities", &m.damage_immunities),
        ("Condition immunities", &m.condition_immunities),
    ] {
        if !list.is_empty() {
            details.push((label, list.join(", ")));
        }
    }
    if !m.senses.is_empty() {
        details.push(("Senses", m.senses.clone()));
    }
    if !m.languages.is_empty() {
        details.push(("Languages", m.languages.clone()));
    }
    details
}

/// A stat block's traits and actions, section by section.
fn features(doc: &mut Doc, m: &MonsterSheet) {
    for (section, features) in [
        (Section::Traits, &m.traits),
        (Section::Actions, &m.actions),
        (Section::BonusActions, &m.bonus_actions),
        (Section::Reactions, &m.reactions),
        (Section::LegendaryActions, &m.legendary_actions),
    ] {
        if features.is_empty() {
            continue;
        }
        doc.heading(section.title());
        for Feature { name, description } in features {
            let mut paragraphs = description.lines().filter(|l| !l.trim().is_empty());
            let first = paragraphs.next().unwrap_or_default();
            doc.bullet(vec![
                strong(format!("{name}.")),
                plain(format!(" {}", dice_together(first))),
            ]);
            for paragraph in paragraphs {
                doc.under_bullet(vec![plain(dice_together(paragraph))]);
            }
        }
    }
}

/// A spell as the books lay it out, `width` columns wide.
pub fn spell(s: &SpellText, width: usize, detail: Detail) -> Vec<Line<'static>> {
    let mut doc = Doc::new(width);
    if detail == Detail::Full {
        doc.line(vec![strong(s.name.clone())]);
    }
    doc.line(vec![
        plain(spell_kind(s)),
        dim(format!("  ·  {}", s.source)),
    ]);
    doc.blank();
    let mut duration = s.duration.clone();
    if s.concentration && !duration.to_lowercase().contains("concentration") {
        duration = format!("Concentration, {}", lowercase_first(&duration));
    }
    let mut components = s.components.clone();
    if !s.material.is_empty() {
        components += &format!(" ({})", s.material);
    }
    let rows = [
        ("Casting time", s.casting_time.clone()),
        ("Range", s.range.clone()),
        ("Components", components),
        ("Duration", duration),
        ("Classes", s.classes.join(", ")),
    ];
    for (label, value) in rows.into_iter().filter(|(_, v)| !v.is_empty()) {
        doc.row(0, label, 12, vec![plain(value)]);
    }
    if !s.description.is_empty() {
        doc.blank();
        rich(&mut doc, &s.description, false);
    }
    if !s.higher_levels.is_empty() {
        doc.heading(if s.level == 0 {
            "Cantrip upgrade"
        } else {
            "At higher levels"
        });
        rich(&mut doc, &s.higher_levels, true);
    }
    doc.into_lines()
}

/// "Level 3 Evocation", or the 2014 books' "3rd-level evocation", with
/// "(ritual)" if it is one.
pub fn spell_kind(s: &SpellText) -> String {
    let school = if s.school.is_empty() {
        "spell".to_string()
    } else {
        s.school.clone()
    };
    let mut kind = match (s.level, s.edition) {
        (0, _) => format!("{school} cantrip"),
        (level, Some(Edition::Rules2014)) => {
            format!("{}-level {}", ordinal(level), school.to_lowercase())
        }
        (level, _) => format!("Level {level} {school}"),
    };
    if s.ritual {
        kind += " (ritual)";
    }
    kind
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A condition's description, `width` columns wide.
pub fn condition(c: &ConditionText, width: usize, detail: Detail) -> Vec<Line<'static>> {
    let mut doc = Doc::new(width);
    if detail == Detail::Full {
        doc.line(vec![strong(c.name.clone())]);
        doc.line(vec![dim(c.source.clone())]);
        doc.blank();
    }
    rich(&mut doc, &c.description, false);
    if detail == Detail::Panel {
        doc.blank();
        doc.line(vec![dim(c.source.clone())]);
    }
    doc.into_lines()
}

/// Rules text: paragraphs one per line, `- ` starting a list item and
/// `**…**` bold. A blank line between paragraphs, none between items;
/// `indented` for text inside a section.
fn rich(doc: &mut Doc, text: &str, indented: bool) {
    let mut previous_item = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let item = line.strip_prefix("- ").or_else(|| line.strip_prefix("* "));
        if previous_item.is_some() && !(previous_item == Some(true) && item.is_some()) {
            doc.blank();
        }
        previous_item = Some(item.is_some());
        match item {
            Some(item) => doc.bullet(bold_runs(item)),
            None if indented => doc.paragraph(bold_runs(line)),
            None => doc.prose(bold_runs(line)),
        }
    }
}

/// `**Bold.** plain` as spans, with dice kept on one line.
fn bold_runs(text: &str) -> Vec<Span<'static>> {
    dice_together(text)
        .split("**")
        .enumerate()
        .filter(|(_, run)| !run.is_empty())
        .map(|(i, run)| {
            if i % 2 == 1 {
                strong(run.to_string())
            } else {
                plain(run.to_string())
            }
        })
        .collect()
}

/// Spells as a table: name, level and school, C for concentration, source.
pub fn spell_list(spells: &[Entry<SpellText>]) -> String {
    if spells.is_empty() {
        return "No spells.".into();
    }
    let width = spells
        .iter()
        .map(|s| chars(&s.text.name))
        .max()
        .unwrap_or(0);
    let kind_width = spells
        .iter()
        .map(|s| chars(&spell_kind(&s.text)))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for entry in spells {
        let s = &entry.text;
        let concentration = if s.concentration { "C" } else { " " };
        let whose = if entry.campaign.is_some() {
            "  (campaign)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<width$}  {:<kind_width$}  {concentration}  {}{whose}",
            s.name,
            spell_kind(s),
            s.source
        );
    }
    out.trim_end().to_string()
}

/// Rules texts other than spells: name and source.
pub fn text_list<T: RulesText>(entries: &[Entry<T>]) -> String {
    if entries.is_empty() {
        return format!("No {}.", T::KIND);
    }
    let width = entries
        .iter()
        .map(|e| chars(e.text.name()))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for entry in entries {
        let whose = if entry.campaign.is_some() {
            "  (campaign)"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<width$}  {}{whose}",
            entry.text.name(),
            entry.text.source()
        );
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
    fn the_panel_finds_every_section_a_sheet_draws() {
        let character: Character = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::nil(), "campaign": uuid::Uuid::nil(), "version": 1,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "name": "Pike", "notes": "Hates goblins",
            "classes": [{ "name": "Cleric", "level": 3, "spellcasting_ability": "wisdom" }],
            "spellcasting": { "spells": [{ "name": "Bless", "level": 1 }] },
            "resources": [{ "name": "Channel Divinity", "max": 1, "recharge": "short_rest" }],
            "inventory": [{ "name": "Mace" }]
        }))
        .expect("a valid character");
        let lines = sheet(&character, 40, Detail::Full);
        for section in [
            Section::Features,
            Section::Inventory,
            Section::Notes,
            Section::Spellcasting,
        ] {
            assert!(
                crate::styled::scroll_to(&lines, Some(section.title()), 3) > 0,
                "{section:?} isn't found"
            );
        }
    }

    fn goblin() -> edda_types::monster::MonsterSheet {
        serde_json::from_value(serde_json::json!({
            "name": "Goblin Boss", "size": "Small", "type": "fey (goblinoid)",
            "alignment": "chaotic neutral", "armor_class": 17, "hit_points": 21,
            "hit_dice": "6d6", "speed": "30 ft.", "challenge": "1", "source": "SRD 5.2",
            "ability_scores": { "strength": 10, "dexterity": 15, "constitution": 10,
                                "intelligence": 10, "wisdom": 8, "charisma": 10 },
            "traits": [{ "name": "Nimble Escape", "description": "Disengages or hides as a bonus action." }],
            "actions": [{ "name": "Multiattack", "description": "Two scimitar attacks." },
                        { "name": "Scimitar", "description": "Hit: 5 (1d6 + 2) Slashing damage." }],
            "reactions": [{ "name": "Redirect Attack", "description": "Swaps places with a goblin." }]
        }))
        .expect("a valid stat block")
    }

    fn text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_stat_block_shows_what_a_fight_needs() {
        let full = text(&monster(&goblin(), 60, Detail::Full));
        for wanted in [
            "Goblin Boss",
            "Small fey (goblinoid), chaotic neutral  ·  SRD 5.2",
            "AC 17   HP 21 (6d6, max 36)   Speed 30 ft.   Initiative +2",
            "DEX 15 (+2)",
            "WIS  8 (-1)",
            // Worked out from the challenge rating, which is all it gives.
            "CR 1 (200 XP)",
            "── Traits",
            "• Nimble Escape. Disengages or hides as a bonus action.",
            "── Actions",
            "• Scimitar. Hit: 5 (1d6 + 2) Slashing damage.",
            "── Reactions",
        ] {
            assert!(full.contains(wanted), "no {wanted:?} in:\n{full}");
        }
        assert!(!full.contains("Bonus actions"), "no empty sections");
        // In a panel: no name (the title has it), and wrapped to fit.
        let panel = text(&monster(&goblin(), 40, Detail::Panel));
        assert!(!panel.starts_with("Goblin Boss"));
        assert!(panel.lines().all(|l| l.chars().count() <= 40), "{panel}");
        assert!(panel.contains("  • Scimitar. Hit: 5 (1d6 + 2) Slashing\n    damage."));
    }

    #[test]
    fn pools_render_as_pips() {
        assert_eq!(pool(Pool { max: 3, used: 1 }), "●●○ 2/3");
        assert_eq!(pool(Pool { max: 20, used: 5 }), "15/20");
    }

    #[test]
    fn dice_stay_on_one_line() {
        assert_eq!(
            dice_together("Hit: 13 (1d10 + 8) Slashing"),
            format!("Hit: 13 (1d10{KEEP}+{KEEP}8) Slashing")
        );
        assert_eq!(dice_together("a + b in 2d6"), "a + b in 2d6");
    }

    #[test]
    fn names_render_nicely() {
        assert_eq!(skill_name(Skill::SleightOfHand), "Sleight of Hand");
        assert_eq!(ordinal(2), "2nd");
    }
}
