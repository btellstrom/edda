//! Drawing the encounter planner.

use campaign_types::{
    Edition,
    encounter::{Difficulty, difficulty},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Wrap},
};

use super::{App, command};

/// Suggestion rows below the command line: every action, and two more.
const SUGGESTIONS: u16 = command::ACTIONS as u16 + 2;
/// The difficulty box's rows.
const DIFFICULTY: u16 = 6;
/// Plans listed at once.
const PLANS: u16 = 6;
/// Rows the plan, difficulty and plans keep, however short the terminal:
/// a few of the plan's entries, all of the difficulty and the plans list.
const MAIN: u16 = 6 + DIFFICULTY + PLANS + 2;
/// The header line, and the command box's borders, line and sentence.
const FIXED: u16 = 1 + 4;
pub fn draw(frame: &mut Frame, app: &App) {
    let height = frame.area().height;
    let suggestions = height.saturating_sub(FIXED + MAIN).min(SUGGESTIONS);
    let [header, main, command] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(MAIN),
        Constraint::Length(4 + suggestions),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(main);
    let plans = (app.encounters.len() as u16).clamp(1, PLANS) + 2;
    let [plan, rating, list] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(DIFFICULTY),
        Constraint::Length(plans),
    ])
    .areas(left);

    draw_header(frame, app, header);
    draw_plan(frame, app, plan);
    draw_difficulty(frame, app, rating);
    draw_plans(frame, app, list);
    draw_stat_block(frame, app, right);
    draw_command(frame, app, command, usize::from(suggestions));
}

fn edition(app: &App) -> Edition {
    app.login.profile.campaign.edition
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let name = app
        .current()
        .map_or("no plan open", |e| e.plan.name.as_str());
    let line = Line::from(vec![
        Span::raw(" Planning ").bold(),
        Span::raw(format!("· {name} · {} rules", edition(app))),
        Span::raw(format!("   you are {}", app.login.session.claims.name)).dark_gray(),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_plan(frame: &mut Frame, app: &App, area: Rect) {
    let Some(open) = app.current() else {
        let text = Text::from(vec![
            Line::from("No plan open.").dark_gray(),
            Line::from("new <name> starts one; open <plan> works on another.").dark_gray(),
        ]);
        frame.render_widget(
            Paragraph::new(text).block(Block::bordered().title(" Plan ")),
            area,
        );
        return;
    };
    let plan = &open.plan;
    let width = plan
        .monsters
        .iter()
        .map(|m| m.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line> = plan
        .monsters
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let challenge = app
                .summary(m.monster)
                .map_or("?".to_string(), |s| s.challenge.clone());
            let mut spans = vec![
                Span::raw(format!("{:>2} ", i + 1)).bold(),
                Span::raw(format!("{:>2} × {:<width$}", m.count, m.name)),
                Span::raw(format!(
                    "  CR {challenge:<4} {:>6} XP",
                    m.xp * u32::from(m.count)
                ))
                .dark_gray(),
            ];
            if m.roll {
                spans.push(Span::raw("  rolled HP").dark_gray());
            }
            if app.summary(m.monster).is_none() {
                spans.push(Span::raw("  not in the monsters any more").red());
            }
            Line::from(spans)
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from("No monsters yet: add [count] <monster>").dark_gray());
    }
    if !plan.notes.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(plan.notes.clone()).italic());
    }
    let title = format!(" {} · {} monsters ", plan.name, plan.monster_count());
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title)),
        area,
    );
}

/// How hard it is, against whom, and why.
fn draw_difficulty(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(" Difficulty ");
    let Some(open) = app.current() else {
        frame.render_widget(block, area);
        return;
    };
    let party = command::party(&open.plan, &app.roster);
    let levels: Vec<u8> = party.iter().map(|c| c.level).collect();
    let d = difficulty(&open.plan, &levels, edition(app));
    let names: Vec<String> = party
        .iter()
        .map(|c| format!("{} {}", c.name, c.level))
        .collect();
    let whole = if open.plan.party.is_empty() {
        " (everyone)"
    } else {
        ""
    };
    let lines = vec![
        Line::from(vec![
            Span::raw("Party  ").dark_gray(),
            Span::raw(if names.is_empty() {
                "no characters in the campaign".to_string()
            } else {
                names.join(" · ")
            }),
            Span::raw(whole).dark_gray(),
        ]),
        Line::from(vec![
            Span::raw("XP     ").dark_gray(),
            Span::raw(xp_line(&open.plan, &d)),
        ]),
        thresholds_line(&d),
        rating_line(&d, !party.is_empty()),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The XP compared with the thresholds, and how it was reached.
fn xp_line(plan: &campaign_types::encounter::EncounterPlan, d: &Difficulty) -> String {
    let base = plan.xp();
    if d.edition == Edition::Rules2014 && d.multiplier != 1.0 {
        format!(
            "{} ({base} × {} for {} monsters)",
            d.xp,
            d.multiplier,
            plan.monster_count()
        )
    } else {
        d.xp.to_string()
    }
}

fn thresholds_line(d: &Difficulty) -> Line<'static> {
    let mut spans = vec![Span::raw("       ")];
    for (i, (name, threshold)) in d.thresholds.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · ").dark_gray());
        }
        let text = format!("{name} {threshold}");
        spans.push(if Some(*name) == d.rating {
            Span::raw(text).green().bold()
        } else if d.xp >= *threshold {
            Span::raw(text)
        } else {
            Span::raw(text).dark_gray()
        });
    }
    Line::from(spans)
}

fn rating_line(d: &Difficulty, has_party: bool) -> Line<'static> {
    let label = Span::raw("Rating ").dark_gray();
    let lowest = d.thresholds.first().map_or("", |(name, _)| *name);
    match d.rating {
        _ if !has_party => Line::from(vec![label, Span::raw("—")]),
        Some(rating) => Line::from(vec![label, Span::raw(rating).bold()]),
        None => Line::from(vec![label, Span::raw(format!("below {lowest}")).bold()]),
    }
}

fn draw_plans(frame: &mut Frame, app: &App, area: Rect) {
    let visible = usize::from(area.height.saturating_sub(2));
    let current = app
        .encounters
        .iter()
        .position(|e| Some(e.id) == app.current);
    let first = current.map_or(0, |i| i.saturating_sub(visible.saturating_sub(1)));
    let mut lines: Vec<Line> = app
        .encounters
        .iter()
        .skip(first)
        .take(visible)
        .map(|e| {
            let line = Line::from(vec![
                Span::raw(if Some(e.id) == app.current {
                    "▶ "
                } else {
                    "  "
                }),
                Span::raw(e.plan.name.clone()),
                Span::raw(format!(
                    "  {} monsters · {} XP",
                    e.plan.monster_count(),
                    e.plan.xp()
                ))
                .dark_gray(),
            ]);
            if Some(e.id) == app.current {
                line.bold()
            } else {
                line
            }
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from("None yet.").dark_gray());
    }
    let title = format!(" Plans ({}) ", app.encounters.len());
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

fn draw_stat_block(frame: &mut Frame, app: &App, area: Rect) {
    let shown = app.shown();
    let (title, lines) = match shown.and_then(|id| app.stat_blocks.get(&id)) {
        Some(monster) => (
            format!(" {} ", monster.sheet.name),
            crate::display::monster(
                &monster.sheet,
                usize::from(area.width.saturating_sub(2)),
                crate::display::Detail::Panel,
            ),
        ),
        None => (
            " Stat block ".to_string(),
            vec![Line::from("A monster's stat block shows here as you type its name.").dark_gray()],
        ),
    };
    let height = usize::from(area.height.saturating_sub(2));
    let scrolled = app.panel.place(shown, 0, lines.len(), height);
    let mut block = Block::bordered().title(title);
    if let Some(hint) = scrolled.hint() {
        block = block.title_bottom(Line::from(hint).dark_gray().right_aligned());
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((u16::try_from(scrolled.offset).unwrap_or(u16::MAX), 0))
            .wrap(Wrap { trim: false })
            .block(block),
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
            None => Line::from("Type an action; Tab completes.").dark_gray(),
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

    let help = " Tab completes · ↑↓ choose · Enter runs · Esc clears · Ctrl-C quits ";
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(help)),
        area,
    );
    let x = area.x + 3 + app.input.chars().count() as u16;
    frame.set_cursor_position((x.min(area.right().saturating_sub(2)), area.y + 1));
}
