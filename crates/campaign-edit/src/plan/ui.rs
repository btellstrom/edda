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

use super::{Screen, command};
use crate::prompt::{self, Shown};

/// Suggestion rows below the command line: every action, and two more.
#[expect(clippy::cast_possible_truncation, reason = "a couple of dozen actions")]
const SUGGESTIONS: u16 = command::ACTIONS as u16 + 2;
/// The difficulty box's rows.
const DIFFICULTY: u16 = 6;
/// Plans listed at once.
const PLANS: u16 = 6;
/// Rows the plan, difficulty and plans keep before the suggestions get any
/// (a few of the plan's entries, all of the difficulty and the plans
/// list); on a terminal too short even for that, they give way too, and
/// only the command line stays.
const MAIN: u16 = 6 + DIFFICULTY + PLANS + 2;
/// The header line, and the command box's borders, line and sentence.
const FIXED: u16 = 1 + 4;

pub fn draw(frame: &mut Frame, screen: &Screen) {
    let height = frame.area().height;
    let suggestions = height.saturating_sub(FIXED + MAIN).min(SUGGESTIONS);
    let [header, main, command] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(4 + suggestions),
    ])
    .areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(main);
    let plans = u16::try_from(screen.encounters.len())
        .unwrap_or(u16::MAX)
        .clamp(1, PLANS)
        + 2;
    let [plan, rating, list] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(DIFFICULTY),
        Constraint::Length(plans),
    ])
    .areas(left);

    draw_header(frame, screen, header);
    draw_plan(frame, screen, plan);
    draw_difficulty(frame, screen, rating);
    draw_plans(frame, screen, list);
    draw_stat_block(frame, screen, right);
    draw_command(frame, screen, command, usize::from(suggestions));
}

fn draw_header(frame: &mut Frame, screen: &Screen, area: Rect) {
    let name = screen
        .current()
        .map_or("no plan open", |e| e.plan.name.as_str());
    let line = Line::from(vec![
        Span::raw(" Planning ").bold(),
        Span::raw(format!("· {name} · {} rules", screen.edition)),
        Span::raw(format!("   you are {}", screen.me)).dark_gray(),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_plan(frame: &mut Frame, screen: &Screen, area: Rect) {
    let Some(open) = screen.current() else {
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
            let challenge = screen
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
            if screen.summary(m.monster).is_none() {
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
fn draw_difficulty(frame: &mut Frame, screen: &Screen, area: Rect) {
    let block = Block::bordered().title(" Difficulty ");
    let Some(open) = screen.current() else {
        frame.render_widget(block, area);
        return;
    };
    let party = command::party(&open.plan, &screen.roster);
    let levels: Vec<u8> = party.iter().map(|c| c.level).collect();
    let d = difficulty(&open.plan, &levels, screen.edition);
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

fn draw_plans(frame: &mut Frame, screen: &Screen, area: Rect) {
    let visible = usize::from(area.height.saturating_sub(2));
    let current = screen
        .encounters
        .iter()
        .position(|e| Some(e.id) == screen.current);
    let first = current.map_or(0, |i| i.saturating_sub(visible.saturating_sub(1)));
    let mut lines: Vec<Line> = screen
        .encounters
        .iter()
        .skip(first)
        .take(visible)
        .map(|e| {
            let line = Line::from(vec![
                Span::raw(if Some(e.id) == screen.current {
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
            if Some(e.id) == screen.current {
                line.bold()
            } else {
                line
            }
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from("None yet.").dark_gray());
    }
    let title = format!(" Plans ({}) ", screen.encounters.len());
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

fn draw_stat_block(frame: &mut Frame, screen: &Screen, area: Rect) {
    let shown = screen.shown();
    let (title, lines) = match shown.and_then(|id| screen.stat_blocks.get(&id)) {
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
    let scrolled = screen.panel.place(shown, 0, lines.len(), height);
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

fn draw_command(frame: &mut Frame, screen: &Screen, area: Rect, rows: usize) {
    let parsed = screen.parsed();
    let shown = Shown {
        sentence: &parsed.sentence,
        problem: parsed.plan.as_ref().err().map(String::as_str),
        suggestions: &screen.suggestions(),
        status: screen.status.as_ref(),
        hint: "Type an action; Tab completes.",
        help: " Tab completes · ↑↓ choose · Enter runs · Esc clears · Ctrl-C quits ",
    };
    prompt::draw(frame, area, &screen.prompt, &shown, rows);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use campaign_types::{
        Character,
        encounter::{Encounter, EncounterPlan, PlannedMonster},
        monster::{Monster, MonsterSheet},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use uuid::Uuid;

    use super::*;
    use crate::prompt::Prompt;

    /// A plan of three goblins, open, for a party of two 3rd-level
    /// characters.
    fn screen() -> Screen {
        let goblin = Monster {
            id: Uuid::new_v4(),
            campaign: None,
            sheet: MonsterSheet {
                name: "Goblin".into(),
                hit_points: 7,
                armor_class: 15,
                challenge: "1/4".into(),
                ..Default::default()
            },
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let character = |name: &str| -> Character {
            serde_json::from_value(serde_json::json!({
                "id": Uuid::new_v4(), "campaign": Uuid::nil(), "version": 1,
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z",
                "name": name, "classes": [{ "name": "Fighter", "level": 3 }]
            }))
            .expect("a valid character")
        };
        let plan = Encounter {
            id: Uuid::new_v4(),
            campaign: Uuid::nil(),
            plan: EncounterPlan {
                name: "Goblin Ambush".into(),
                monsters: vec![PlannedMonster {
                    monster: goblin.id,
                    name: "Goblin".into(),
                    count: 3,
                    xp: 50,
                    roll: false,
                }],
                ..Default::default()
            },
            version: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        Screen {
            current: Some(plan.id),
            encounters: vec![plan],
            bestiary: vec![(&goblin).into()],
            roster: vec![(&character("Grog")).into(), (&character("Pike")).into()],
            stat_blocks: HashMap::from([(goblin.id, goblin)]),
            prompt: Prompt::default(),
            status: None,
            panel: Default::default(),
            me: "DM laptop".into(),
            edition: Edition::Rules2024,
        }
    }

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

    #[test]
    fn the_open_plan_its_difficulty_and_the_plans_are_shown() {
        let text = render(&screen(), 110, 40);
        for wanted in [
            "Goblin Ambush",
            "3 × Goblin",
            "150 XP",
            "Grog",
            "you are DM laptop",
        ] {
            assert!(text.contains(wanted), "no {wanted:?} in:\n{text}");
        }
    }

    #[test]
    fn a_monster_being_added_shows_its_stat_block() {
        let mut screen = screen();
        let before = render(&screen, 110, 40);
        assert!(!before.contains("AC 15"), "no stat block yet");
        screen.prompt.input = "add 2 goblin".into();
        let text = render(&screen, 110, 40);
        assert!(text.contains("AC 15"), "{text}");
        assert!(text.contains("Add 2 × Goblin"), "the line in words");
    }

    #[test]
    fn a_cramped_terminal_still_has_the_command_line() {
        let mut screen = screen();
        screen.prompt.input = "add goblin".into();
        for (width, height) in [(80, 24), (60, 12), (30, 6)] {
            let text = render(&screen, width, height);
            assert!(text.contains("> add goblin"), "{width}×{height}:\n{text}");
        }
    }
}
