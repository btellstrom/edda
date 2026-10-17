//! Drawing the fight screen.

use edda_types::{
    combat::{CombatCommand, CombatState, CombatStatus, Combatant, Effect, Vitality},
    rules_text,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Wrap},
};
use uuid::Uuid;

use super::{
    Screen,
    command::{self, Step, Topic},
    log,
};
use crate::{
    display::{self, Detail, Section},
    prompt::{self, Shown},
    styled::{Doc, dim, plain},
};

/// Suggestion rows below the command line: every action (what an empty
/// line lists), and two more.
#[expect(clippy::cast_possible_truncation, reason = "a couple of dozen actions")]
const SUGGESTIONS: u16 = command::ACTIONS as u16 + 2;
/// Rows the initiative order and sheet keep before the log or suggestions
/// get any; on a terminal too short even for that, they give way too, and
/// only the command line stays.
const MAIN: u16 = 8;
/// The log's rows, and how few it may shrink to.
const LOG: u16 = 8;
const LOG_MIN: u16 = 4;
/// The header line, and the command box's borders, line and sentence.
const FIXED: u16 = 1 + 4;

/// Rows for the log and the suggestions on a terminal `height` rows tall.
/// On a short one the log gives way first, then the suggestions; the log
/// goes altogether rather than be too short to read.
fn rows(height: u16) -> (u16, u16) {
    let spare = height.saturating_sub(FIXED + MAIN);
    let log = if spare < LOG_MIN {
        0
    } else {
        spare.saturating_sub(SUGGESTIONS).clamp(LOG_MIN, LOG)
    };
    let suggestions = spare.saturating_sub(log).min(SUGGESTIONS);
    (log, suggestions)
}

pub fn draw(frame: &mut Frame, screen: &Screen) {
    let (log_rows, suggestion_rows) = rows(frame.area().height);
    let [header, main, history, command] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(log_rows),
        Constraint::Length(4 + suggestion_rows),
    ])
    .areas(frame.area());
    let [order, sheet] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main);

    draw_header(frame, screen, header);
    draw_order(frame, screen, order);
    draw_panel(frame, screen, sheet);
    draw_log(frame, screen, history);
    draw_command(frame, screen, command, usize::from(suggestion_rows));
}

fn draw_header(frame: &mut Frame, screen: &Screen, area: Rect) {
    let state = screen.replica.state();
    let status = match state.status {
        CombatStatus::Preparing => "preparing".to_string(),
        CombatStatus::Active => format!("round {}", state.round),
        CombatStatus::Paused => format!("round {} · paused", state.round),
        CombatStatus::Ended => "ended".to_string(),
    };
    let connection = if screen.online {
        Span::raw("")
    } else {
        Span::raw("  offline").red().bold()
    };
    let line = Line::from(vec![
        Span::raw(format!(" {} ", state.name)).bold(),
        Span::raw(format!("· {status}")),
        connection,
        Span::raw(format!("   you are {}", screen.me)).dark_gray(),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// Combatants a parsed line would affect.
fn targets(steps: &[Step]) -> Vec<Uuid> {
    let mut out = Vec::new();
    for step in steps {
        let Step::Combat(command) = step else {
            continue;
        };
        match command {
            CombatCommand::Damage { combatant, .. }
            | CombatCommand::Heal { combatant, .. }
            | CombatCommand::SetTemporaryHitPoints { combatant, .. }
            | CombatCommand::SetInitiative { combatant, .. }
            | CombatCommand::RollInitiative { combatant, .. }
            | CombatCommand::RemoveCombatant { combatant } => out.push(*combatant),
            CombatCommand::AddEffect { effect } => out.extend(&effect.targets),
            CombatCommand::CastSpell { targets, .. } => out.extend(targets),
            _ => {}
        }
    }
    out
}

fn draw_order(frame: &mut Frame, screen: &Screen, area: Rect) {
    let state = screen.replica.state();
    let parsed = screen.parsed();
    let targeted = parsed.plan.as_deref().map(targets).unwrap_or_default();
    let width = state
        .combatants
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(0);

    let mut lines = Vec::new();
    for (i, c) in state.combatants.iter().enumerate() {
        let turn = state.current_turn == Some(c.id);
        let marker = if turn { "▶" } else { " " };
        let initiative = c
            .initiative
            .map_or("  —".to_string(), |i| format!("{i:>3}"));
        let mut style = Style::new();
        if c.is_down() {
            style = style.fg(Color::Red);
        }
        if turn {
            style = style.add_modifier(Modifier::BOLD);
        }
        let mut role = Span::raw("");
        if parsed.actor == Some(c.id) && !screen.prompt.input.is_empty() {
            style = style.fg(Color::Cyan);
            role = Span::raw("  acts").cyan();
        }
        if targeted.contains(&c.id) {
            style = style.fg(Color::Magenta);
            role = Span::raw("  ← target").magenta();
        }
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{:>2} ", i + 1), style),
            Span::styled(format!("{:<width$}", c.name), style),
            Span::raw(format!(" {initiative}  ")),
            hp(c),
            c.armor_class.map_or(Span::raw(""), |ac| {
                Span::raw(format!("  AC {ac}")).dark_gray()
            }),
            role,
        ]));
        for effect in state.effects.iter().filter(|e| e.targets.contains(&c.id)) {
            lines.push(
                Line::from(format!("      {}{}", effect.name, time_left(effect, state)))
                    .dark_gray()
                    .italic(),
            );
        }
    }
    let untargeted: Vec<_> = state
        .effects
        .iter()
        .filter(|e| e.targets.is_empty())
        .collect();
    if !untargeted.is_empty() {
        lines.push(Line::raw(""));
        for effect in untargeted {
            let source = effect
                .source
                .and_then(|s| state.combatant(s))
                .map_or(String::new(), |s| format!(" ({})", s.name));
            let left = time_left(effect, state);
            lines.push(Line::from(format!("  {}{source}{left}", effect.name)).dark_gray());
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("  Nobody yet: add <character> or add <name> <hp>").dark_gray());
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Initiative ")),
        area,
    );
}

/// How long an effect has left, counted in rounds until the turn it ends on.
fn time_left(effect: &Effect, state: &CombatState) -> String {
    let Some(expiry) = effect.expires else {
        return String::new();
    };
    let left = expiry.round.saturating_sub(state.round);
    let whose = expiry
        .at_turn_of
        .and_then(|id| state.combatant(id))
        .map(|c| c.name.as_str());
    match (left, whose) {
        (0, Some(whose)) => format!(" · ends on {whose}'s turn"),
        (0, None) => " · ends this round".into(),
        (1, _) => " · 1 round left".into(),
        (n, _) => format!(" · {n} rounds left"),
    }
}

fn hp(c: &Combatant) -> Span<'static> {
    if c.concealed {
        // Behind the DM's screen: only how hurt it looks.
        let vitality = c.vitality();
        let text = format!("{vitality:<7}");
        return match vitality {
            Vitality::Down => Span::raw(text).red(),
            Vitality::Bloodied => Span::raw(text).yellow(),
            Vitality::Hurt | Vitality::Unhurt => Span::raw(text).green(),
        };
    }
    let hp = c.hit_points;
    let temp = if hp.temporary > 0 {
        format!(" +{}", hp.temporary)
    } else {
        String::new()
    };
    let text = format!("{:>3}/{:<3}{temp}", hp.current, hp.max);
    let ratio = f32::from(hp.current) / f32::from(hp.max.max(1));
    if hp.current == 0 {
        Span::raw(text).red()
    } else if ratio <= 0.5 {
        Span::raw(text).yellow()
    } else {
        Span::raw(text).green()
    }
}

/// The side panel: a description of what the line is about (or what
/// `show` keeps there), else the acting combatant's sheet.
fn draw_panel(frame: &mut Frame, screen: &Screen, area: Rect) {
    let state = screen.replica.state();
    let ctx = screen.context();
    let topic = screen.topic().map(|topic| match topic {
        // A name that's only a combatant's is their sheet.
        Topic::Named(name) if !describes_text(screen, &name) => ctx
            .named_combatant(&name)
            .map_or(Topic::Named(name), |c| Topic::Combatant(c.id)),
        other => other,
    });
    match topic {
        Some(Topic::Combatant(id)) => {
            if let Some(combatant) = state.combatant(id) {
                draw_sheet(frame, screen, area, combatant);
            }
        }
        Some(topic) => draw_description(frame, screen, area, &topic),
        None => match screen.shown() {
            Some(combatant) => draw_sheet(frame, screen, area, combatant),
            None => frame.render_widget(
                Paragraph::new("Nobody's turn yet.")
                    .dark_gray()
                    .block(Block::bordered().title(" Sheet ")),
                area,
            ),
        },
    }
}

/// Whether `name` is an effect in play, a spell or a condition.
fn describes_text(screen: &Screen, name: &str) -> bool {
    let ctx = screen.context();
    ctx.describes(&Topic::Effect(name.to_string()))
        || ctx.describes(&Topic::Spell(name.to_string()))
}

/// A spell's, effect's or condition's description, with how an effect in
/// play stands and, while casting, the caster's slots.
fn draw_description(frame: &mut Frame, screen: &Screen, area: Rect, topic: &Topic) {
    let inner = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let state = screen.replica.state();
    let name = match topic {
        Topic::Spell(name) | Topic::Effect(name) | Topic::Named(name) => name.trim(),
        Topic::Combatant(_) => return,
    };
    let effect = match topic {
        Topic::Spell(_) => None,
        _ => state
            .effects
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name)),
    };
    let mut doc = Doc::new(inner);
    if let Some(effect) = effect {
        effect_rows(&mut doc, effect, state);
        doc.blank();
    }
    if let Topic::Spell(_) = topic {
        // What there is to cast it with.
        let slots = screen
            .shown()
            .and_then(|c| c.character_id)
            .and_then(|id| screen.sheets.get(&id))
            .and_then(|c| display::slots_line(&c.sheet));
        if let Some(slots) = slots {
            doc.row(0, "Slots", 5, vec![plain(slots)]);
            doc.blank();
        }
    }
    let mut lines = doc.into_lines();
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let spell = rules_text::named(&screen.spells, name);
    let condition = match topic {
        Topic::Spell(_) => None,
        _ => rules_text::named(&screen.conditions, name),
    };
    let (title, text) = if let Some(spell) = spell {
        (
            spell.text.name.clone(),
            display::spell(&spell.text, inner, Detail::Panel),
        )
    } else if let Some(condition) = condition {
        (
            condition.text.name.clone(),
            display::condition(&condition.text, inner, Detail::Panel),
        )
    } else {
        let known = screen.context().known_spell(name).cloned();
        let mut doc = Doc::new(inner);
        let title = match &known {
            Some(spell) => {
                let mut kind = if spell.level == 0 {
                    "Cantrip".to_string()
                } else {
                    format!("Level {} spell", spell.level)
                };
                if spell.concentration {
                    kind += ", concentration";
                }
                if spell.ritual {
                    kind += ", ritual";
                }
                doc.line(vec![plain(kind)]);
                if !spell.notes.is_empty() {
                    doc.blank();
                    doc.prose(vec![plain(spell.notes.clone())]);
                }
                doc.blank();
                spell.name.clone()
            }
            None => effect.map_or(name.to_string(), |e| e.name.clone()),
        };
        let what = if known.is_some() {
            "spell"
        } else {
            "condition"
        };
        doc.prose(vec![dim(format!(
            "No description of {title} in the campaign; `edda {what} add` can add one."
        ))]);
        (title, doc.into_lines())
    };
    lines.extend(text);
    let title = if effect.is_some() {
        format!(" {title} · in play ")
    } else {
        format!(" {title} ")
    };
    let scrolled = screen.panel.place(topic, 0, lines.len(), height);
    render_panel(frame, area, title, Text::from(lines), scrolled);
}

/// Who an effect in play is on, who put it there, and how long it lasts.
fn effect_rows(doc: &mut Doc, effect: &Effect, state: &CombatState) {
    let named = |id: &Uuid| state.combatant(*id).map(|c| c.name.clone());
    let on: Vec<String> = effect.targets.iter().filter_map(named).collect();
    doc.row(
        0,
        "On",
        5,
        vec![plain(if on.is_empty() {
            "nobody: it's put down".to_string()
        } else {
            on.join(", ")
        })],
    );
    if let Some(source) = effect.source.as_ref().and_then(named) {
        let concentrating = if effect.concentration {
            ", concentrating"
        } else {
            ""
        };
        doc.row(0, "By", 5, vec![plain(format!("{source}{concentrating}"))]);
    }
    let left = time_left(effect, state);
    let left = left.trim_start_matches(" · ");
    doc.row(
        0,
        "Lasts",
        5,
        vec![plain(if left.is_empty() {
            "until removed".to_string()
        } else {
            left.to_string()
        })],
    );
}

fn render_panel(
    frame: &mut Frame,
    area: Rect,
    title: String,
    text: Text<'static>,
    scrolled: crate::styled::Scrolled,
) {
    let mut block = Block::bordered().title(title);
    if let Some(hint) = scrolled.hint() {
        block = block.title_bottom(Line::from(hint).dark_gray().right_aligned());
    }
    frame.render_widget(
        Paragraph::new(text)
            .scroll((u16::try_from(scrolled.offset).unwrap_or(u16::MAX), 0))
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn draw_sheet(frame: &mut Frame, screen: &Screen, area: Rect, combatant: &Combatant) {
    // Inside the borders.
    let inner = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let sheet = combatant.character_id.and_then(|id| screen.sheets.get(&id));
    let monster = combatant.monster_id.and_then(|id| screen.monsters.get(&id));
    let text = match (sheet, monster) {
        // A player's device: the numbers are behind the DM's screen, and
        // its stat block with them.
        _ if combatant.concealed => Text::from(vec![
            Line::from(combatant.name.clone()).bold(),
            Line::raw(""),
            Line::raw(format!("Looks {}", combatant.vitality())),
            Line::raw(""),
            Line::from("The rest is behind the DM's screen.").dark_gray(),
        ]),
        (None, Some(monster)) => {
            // Its own hit points first; the stat block's are what it
            // started from.
            let hp = combatant.hit_points;
            let temp = if hp.temporary > 0 {
                format!(" +{} temporary", hp.temporary)
            } else {
                String::new()
            };
            let mut lines = vec![
                Line::from(format!("HP {}/{}{temp}", hp.current, hp.max)).bold(),
                Line::raw(""),
            ];
            lines.extend(crate::display::monster(
                &monster.sheet,
                inner,
                crate::display::Detail::Panel,
            ));
            Text::from(lines)
        }
        (sheet, _) => match sheet {
            Some(character) => {
                // The combat's hit points are the current ones; the sheet's
                // catch up a moment later.
                let mut character = character.clone();
                character.sheet.hit_points = combatant.hit_points;
                Text::from(crate::display::sheet(
                    &character,
                    inner,
                    crate::display::Detail::Panel,
                ))
            }
            None => {
                let hp = combatant.hit_points;
                let mut lines = vec![
                    Line::from(combatant.name.clone()).bold(),
                    Line::raw(""),
                    Line::raw(format!("HP {}/{}", hp.current, hp.max)),
                ];
                if hp.temporary > 0 {
                    lines.push(Line::raw(format!("Temporary HP {}", hp.temporary)));
                }
                if let Some(ac) = combatant.armor_class {
                    lines.push(Line::raw(format!("AC {ac}")));
                }
                lines.push(Line::raw(format!(
                    "Initiative modifier {:+}",
                    combatant.initiative_modifier
                )));
                lines.push(Line::raw(""));
                lines.push(Line::from("No character sheet.").dark_gray());
                Text::from(lines)
            }
        },
    };
    // When it doesn't all fit, show the part the line needs: spells for a
    // cast, a monster's actions for an attack.
    let section = screen.parsed().section;
    let automatic = crate::styled::scroll_to(&text.lines, section.map(Section::title), height);
    let scrolled = screen
        .panel
        .place((combatant.id, section), automatic, text.lines.len(), height);
    let turn = screen.replica.state().current_turn == Some(combatant.id);
    let title = if turn {
        format!(" {} · their turn ", combatant.name)
    } else {
        format!(" {} ", combatant.name)
    };
    render_panel(frame, area, title, text, scrolled);
}

fn draw_log(frame: &mut Frame, screen: &Screen, area: Rect) {
    // Who did it, on the first line of each command.
    let mut lines: Vec<Line> = Vec::new();
    let mut previous = None;
    for event in &screen.log {
        let first = event.command_id.is_none() || event.command_id != previous;
        previous = event.command_id;
        let actor = match (&event.actor, first) {
            (Some(actor), true) => format!("{actor}: "),
            (Some(actor), false) => " ".repeat(actor.chars().count() + 2),
            (None, _) => String::new(),
        };
        lines.push(Line::from(vec![
            Span::raw(actor).dark_gray(),
            Span::raw(log::describe_in(&event.event, &screen.log, &screen.names)),
        ]));
    }
    let visible = usize::from(area.height.saturating_sub(2));
    let end = lines.len().saturating_sub(screen.scroll.min(lines.len()));
    let start = end.saturating_sub(visible);
    let title = if screen.scroll > 0 {
        " Log (scrolled back; PgDn) "
    } else {
        " Log "
    };
    frame.render_widget(
        Paragraph::new(lines[start..end].to_vec()).block(Block::bordered().title(title)),
        area,
    );
}

fn draw_command(frame: &mut Frame, screen: &Screen, area: Rect, rows: usize) {
    let parsed = screen.parsed();
    let shown = Shown {
        sentence: &parsed.sentence,
        problem: parsed.plan.as_ref().err().map(String::as_str),
        suggestions: &screen.suggestions(),
        status: screen.status.as_ref(),
        hint: "Type an action, or a number for who acts.",
        help: " Tab completes · ↑↓ choose · Enter runs · Esc clears · PgUp log · Ctrl-C quits ",
    };
    prompt::draw(frame, area, &screen.prompt, &shown, rows);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use edda_client::combat::Replica;
    use edda_types::{
        Character,
        combat::{CombatEvent, CombatSnapshot, Concealer, Effect},
        monster::{Monster, MonsterSheet},
    };
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::{fight::log::Names, prompt::Prompt};

    /// Vex (a ranger with spells) has the turn against a goblin, who is
    /// hurt and marked by her Hunter's Mark.
    fn screen(player: bool) -> (Screen, Uuid) {
        let vex: Character = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "campaign": Uuid::nil(), "version": 1,
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
            "owners": ["laura"],
            "name": "Vex", "race": "Half-Elf", "armor_class": 16,
            "hit_points": { "current": 44, "max": 44 },
            "classes": [{ "name": "Ranger", "level": 5, "spellcasting_ability": "wisdom" }],
            "spellcasting": { "spells": [
                { "name": "Hunter's Mark", "level": 1, "concentration": true, "prepared": true },
                { "name": "Cure Wounds", "level": 1, "prepared": true }
            ] },
            "resources": [{ "name": "Favored Foe", "max": 3, "recharge": "long_rest" }],
            "inventory": [{ "name": "Longbow" }, { "name": "Arrows", "quantity": 40 }],
            "notes": "Trusts Trinket more than people."
        }))
        .expect("a valid character");
        let goblin = Monster {
            id: Uuid::new_v4(),
            campaign: None,
            sheet: MonsterSheet {
                name: "Goblin".into(),
                hit_points: 12,
                armor_class: 15,
                ..Default::default()
            },
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let mut vex_c = Combatant::from_character(&vex);
        vex_c.initiative = Some(20);
        let mut goblin_c = Combatant::from_monster(&goblin, "Goblin".into(), 12);
        goblin_c.initiative = Some(5);
        let goblin_id = goblin_c.id;
        let mut state = CombatState::default();
        for event in [
            CombatEvent::CombatantAdded {
                combatant: vex_c.clone(),
            },
            CombatEvent::CombatantAdded {
                combatant: goblin_c,
            },
            CombatEvent::Started,
            CombatEvent::RoundStarted { round: 1 },
            CombatEvent::TurnStarted {
                combatant: vex_c.id,
            },
            CombatEvent::Damaged {
                combatant: goblin_id,
                amount: 5,
                hit_points: edda_types::HitPoints {
                    max: 12,
                    current: 7,
                    temporary: 0,
                },
            },
            CombatEvent::EffectAdded {
                effect: Effect {
                    id: Uuid::new_v4(),
                    name: "Hunter's Mark".into(),
                    source: Some(vex_c.id),
                    targets: vec![goblin_id],
                    concentration: true,
                    expires: None,
                },
            },
        ] {
            state.apply(&event);
        }
        if player {
            state = Concealer::new(&state).state(&state);
        }
        let screen = Screen {
            replica: Replica::new(CombatSnapshot {
                id: Uuid::nil(),
                seq: 7,
                state,
            }),
            log: Vec::new(),
            names: Names::default(),
            roster: vec![(&vex).into()],
            sheets: HashMap::from([(vex.id, vex)]),
            bestiary: Vec::new(),
            monsters: HashMap::from([(goblin.id, goblin)]),
            spells: Vec::new(),
            conditions: Vec::new(),
            pinned: None,
            prompt: Prompt::default(),
            status: None,
            scroll: 0,
            panel: Default::default(),
            player: player.then(|| "laura".to_string()),
            me: if player { "Laura's phone" } else { "DM laptop" }.into(),
            online: true,
        };
        (screen, goblin_id)
    }

    /// The screen drawn on a terminal `width` by `height`, as text.
    fn render(screen: &Screen, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("in memory");
        terminal
            .draw(|frame| draw(frame, screen))
            .expect("in memory");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_with<'a>(text: &'a str, wanted: &str) -> &'a str {
        text.lines()
            .find(|l| l.contains(wanted))
            .unwrap_or_else(|| panic!("no line with {wanted:?} in:\n{text}"))
    }

    #[test]
    fn the_order_shows_whose_turn_it_is_and_how_everyone_stands() {
        let (screen, _) = screen(false);
        let text = render(&screen, 110, 40);
        let vex = line_with(&text, "1 Vex");
        assert!(
            vex.contains('▶') && vex.contains("44/44") && vex.contains("AC 16"),
            "{vex}"
        );
        let goblin = line_with(&text, "2 Goblin");
        assert!(
            goblin.contains("7/12") && goblin.contains("AC 15"),
            "{goblin}"
        );
        assert!(
            text.contains("Hunter's Mark"),
            "the effect under its target"
        );
        assert!(text.contains("you are DM laptop"));
        assert!(text.contains("Type an action, or a number for who acts."));
    }

    #[test]
    fn a_player_sees_monsters_only_by_how_hurt_they_look() {
        let (mut screen, goblin) = screen(true);
        let text = render(&screen, 110, 40);
        let row = line_with(&text, "2 Goblin");
        assert!(
            row.contains("hurt") && !row.contains("7/12") && !row.contains("AC"),
            "{row}"
        );
        assert!(
            line_with(&text, "1 Vex").contains("44/44"),
            "the party's own numbers"
        );

        screen.pinned = Some(Topic::Combatant(goblin));
        let text = render(&screen, 110, 40);
        assert!(text.contains("Looks hurt"));
        assert!(text.contains("The rest is behind the DM's screen."));
        assert!(!text.contains("AC 15"), "nor the stat block");
    }

    #[test]
    fn typing_a_cast_turns_the_panel_to_spellcasting_and_lists_spells() {
        let (mut screen, _) = screen(false);
        // Short enough that the sheet doesn't fit.
        let before = render(&screen, 110, 30);
        assert!(
            !before.contains("── Spellcasting"),
            "the panel starts at the top"
        );
        screen.prompt.input = "cast ".into();
        let text = render(&screen, 110, 30);
        assert!(text.contains("── Spellcasting"), "{text}");
        assert!(
            text.lines()
                .any(|l| l.contains("Cure Wounds") && l.contains("1st level")),
            "the spells as suggestions"
        );
        assert!(text.contains("Vex casts …"), "the line in words");
    }

    #[test]
    fn a_cramped_terminal_still_has_the_command_line() {
        let (mut screen, _) = screen(false);
        screen.prompt.input = "damage 2 3".into();
        for (width, height) in [(60, 16), (40, 10), (20, 6)] {
            let text = render(&screen, width, height);
            assert!(text.contains("> damage"), "{width}×{height}:\n{text}");
        }
    }

    #[test]
    fn every_action_fits_with_two_rows_to_spare_when_there_is_room() {
        assert_eq!(SUGGESTIONS as usize, command::ACTIONS + 2);
        assert_eq!(rows(60), (LOG, SUGGESTIONS));
        // A 40-row terminal gives up two rows of log for them.
        assert_eq!(rows(40), (6, SUGGESTIONS));
        // Shorter: the log shrinks to its least, then the suggestions.
        assert_eq!(rows(35), (LOG_MIN, 18));
        assert_eq!(rows(20), (LOG_MIN, 3));
        assert_eq!(rows(16), (0, 3), "no log rather than one too short to read");
        assert_eq!(rows(10), (0, 0));
    }
}
