//! Drawing the fight screen.

use campaign_types::{
    combat::{CombatCommand, CombatState, CombatStatus, Combatant, Effect},
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
    App,
    command::{self, Step, Topic},
    log,
};
use crate::{
    display::{self, Detail},
    styled::{Doc, dim, plain},
};

/// Suggestion rows below the command line: every action (what an empty
/// line lists), and two more.
#[expect(clippy::cast_possible_truncation, reason = "a couple of dozen actions")]
const SUGGESTIONS: u16 = command::ACTIONS as u16 + 2;
/// Rows the initiative order and sheet keep, however short the terminal.
const MAIN: u16 = 8;
/// The log's rows, and how few it may shrink to.
const LOG: u16 = 8;
const LOG_MIN: u16 = 4;
/// The header line, and the command box's borders, line and sentence.
const FIXED: u16 = 1 + 4;

/// Rows for the log and the suggestions on a terminal `height` rows tall.
/// On a short one the log gives way first, then the suggestions.
fn rows(height: u16) -> (u16, u16) {
    let spare = height.saturating_sub(FIXED + MAIN);
    let log = spare.saturating_sub(SUGGESTIONS).clamp(LOG_MIN, LOG);
    let suggestions = spare.saturating_sub(log).min(SUGGESTIONS);
    (log, suggestions)
}

pub fn draw(frame: &mut Frame, app: &App) {
    let (log_rows, suggestion_rows) = rows(frame.area().height);
    let [header, main, history, command] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(MAIN),
        Constraint::Length(log_rows),
        Constraint::Length(4 + suggestion_rows),
    ])
    .areas(frame.area());
    let [order, sheet] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main);

    draw_header(frame, app, header);
    draw_order(frame, app, order);
    draw_panel(frame, app, sheet);
    draw_log(frame, app, history);
    draw_command(frame, app, command, usize::from(suggestion_rows));
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let state = app.replica.state();
    let status = match state.status {
        CombatStatus::Preparing => "preparing".to_string(),
        CombatStatus::Active => format!("round {}", state.round),
        CombatStatus::Paused => format!("round {} · paused", state.round),
        CombatStatus::Ended => "ended".to_string(),
    };
    let connection = if app.sender.is_some() {
        Span::raw("")
    } else {
        Span::raw("  offline").red().bold()
    };
    let line = Line::from(vec![
        Span::raw(format!(" {} ", state.name)).bold(),
        Span::raw(format!("· {status}")),
        connection,
        Span::raw(format!("   you are {}", app.login.session.claims.name)).dark_gray(),
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

fn draw_order(frame: &mut Frame, app: &App, area: Rect) {
    let state = app.replica.state();
    let parsed = app.parsed();
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
        if parsed.actor == Some(c.id) && !app.input.is_empty() {
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
fn draw_panel(frame: &mut Frame, app: &App, area: Rect) {
    let state = app.replica.state();
    let ctx = app.context();
    let topic = app.topic().map(|topic| match topic {
        // A name that's only a combatant's is their sheet.
        Topic::Named(name) if !describes_text(app, &name) => ctx
            .named_combatant(&name)
            .map_or(Topic::Named(name), |c| Topic::Combatant(c.id)),
        other => other,
    });
    match topic {
        Some(Topic::Combatant(id)) => {
            if let Some(combatant) = state.combatant(id) {
                draw_sheet(frame, app, area, combatant);
            }
        }
        Some(topic) => draw_description(frame, app, area, &topic),
        None => match app.shown() {
            Some(combatant) => draw_sheet(frame, app, area, combatant),
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
fn describes_text(app: &App, name: &str) -> bool {
    let ctx = app.context();
    ctx.describes(&Topic::Effect(name.to_string()))
        || ctx.describes(&Topic::Spell(name.to_string()))
}

/// A spell's, effect's or condition's description, with how an effect in
/// play stands and, while casting, the caster's slots.
fn draw_description(frame: &mut Frame, app: &App, area: Rect, topic: &Topic) {
    let inner = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let state = app.replica.state();
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
        let slots = app
            .shown()
            .and_then(|c| c.character_id)
            .and_then(|id| app.sheets.get(&id))
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
    let spell = rules_text::named(&app.spells, name);
    let condition = match topic {
        Topic::Spell(_) => None,
        _ => rules_text::named(&app.conditions, name),
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
        let known = app.context().known_spell(name).cloned();
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
            "No description of {title} in the campaign; `campaign-edit {what} add` can add one."
        ))]);
        (title, doc.into_lines())
    };
    lines.extend(text);
    let title = if effect.is_some() {
        format!(" {title} · in play ")
    } else {
        format!(" {title} ")
    };
    let scrolled = app.panel.place(topic, 0, lines.len(), height);
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

fn draw_sheet(frame: &mut Frame, app: &App, area: Rect, combatant: &Combatant) {
    // Inside the borders.
    let inner = usize::from(area.width.saturating_sub(2));
    let height = usize::from(area.height.saturating_sub(2));
    let sheet = combatant.character_id.and_then(|id| app.sheets.get(&id));
    let monster = combatant.monster_id.and_then(|id| app.monsters.get(&id));
    let text = match (sheet, monster) {
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
    let section = app.parsed().section;
    let automatic = crate::styled::scroll_to(&text.lines, section, height);
    let scrolled = app
        .panel
        .place((combatant.id, section), automatic, text.lines.len(), height);
    let turn = app.replica.state().current_turn == Some(combatant.id);
    let title = if turn {
        format!(" {} · their turn ", combatant.name)
    } else {
        format!(" {} ", combatant.name)
    };
    render_panel(frame, area, title, text, scrolled);
}

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    // Who did it, on the first line of each command.
    let mut lines: Vec<Line> = Vec::new();
    let mut previous = None;
    for event in &app.log {
        let first = event.command_id.is_none() || event.command_id != previous;
        previous = event.command_id;
        let actor = match (&event.actor, first) {
            (Some(actor), true) => format!("{actor}: "),
            (Some(actor), false) => " ".repeat(actor.chars().count() + 2),
            (None, _) => String::new(),
        };
        lines.push(Line::from(vec![
            Span::raw(actor).dark_gray(),
            Span::raw(log::describe_in(&event.event, &app.log, &app.names)),
        ]));
    }
    let visible = usize::from(area.height.saturating_sub(2));
    let end = lines.len().saturating_sub(app.scroll.min(lines.len()));
    let start = end.saturating_sub(visible);
    let title = if app.scroll > 0 {
        " Log (scrolled back; PgDn) "
    } else {
        " Log "
    };
    frame.render_widget(
        Paragraph::new(lines[start..end].to_vec()).block(Block::bordered().title(title)),
        area,
    );
}

fn draw_command(frame: &mut Frame, app: &App, area: Rect, rows: usize) {
    let parsed = app.parsed();
    let mut lines = vec![Line::from(vec![
        Span::raw("> ").bold(),
        Span::raw(app.input.clone()),
    ])];
    lines.push(if app.input.is_empty() {
        match &app.status {
            Some((text, true)) => Line::from(text.clone()).red(),
            Some((text, false)) => Line::from(format!("✓ {text}")).green(),
            None => Line::from("Type an action, or a number for who acts.").dark_gray(),
        }
    } else {
        match &parsed.plan {
            Ok(_) => Line::from(vec![
                Span::raw(parsed.sentence.clone()).bold(),
                Span::raw("   ⏎").green(),
            ]),
            Err(problem) => Line::from(vec![
                Span::raw(parsed.sentence.clone()),
                Span::raw(if parsed.sentence.is_empty() {
                    ""
                } else {
                    "   "
                }),
                Span::raw(problem.clone()).yellow(),
            ]),
        }
    });

    let suggestions = app.suggestions();
    let first = app.selected.saturating_sub(rows.saturating_sub(1));
    let width = suggestions
        .iter()
        .map(|s| s.label.chars().count())
        .max()
        .unwrap_or(0);
    for (i, s) in suggestions.iter().enumerate().skip(first).take(rows) {
        let line = Line::from(vec![
            Span::raw(format!("  {:<width$}  ", s.label)),
            Span::raw(s.detail.clone()).dark_gray(),
        ]);
        lines.push(if i == app.selected {
            line.reversed()
        } else {
            line
        });
    }

    let help = " Tab completes · ↑↓ choose · Enter runs · Esc clears · PgUp log · Ctrl-C quits ";
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(help)),
        area,
    );
    let typed = u16::try_from(app.input.chars().count()).unwrap_or(u16::MAX);
    let x = area.x.saturating_add(3).saturating_add(typed);
    frame.set_cursor_position((x.min(area.right().saturating_sub(2)), area.y + 1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_fits_with_two_rows_to_spare_when_there_is_room() {
        assert_eq!(SUGGESTIONS as usize, command::ACTIONS + 2);
        assert_eq!(rows(60), (LOG, SUGGESTIONS));
        // A 40-row terminal gives up two rows of log for them.
        assert_eq!(rows(40), (6, SUGGESTIONS));
        // Shorter: the log shrinks to its least, then the suggestions.
        assert_eq!(rows(35), (LOG_MIN, 18));
        assert_eq!(rows(20), (LOG_MIN, 3));
        assert_eq!(rows(10), (LOG_MIN, 0));
    }
}
