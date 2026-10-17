//! The command line both screens are driven from: the line being typed,
//! the keys that edit it, and the box it's drawn in, with what the line
//! means and what could come next.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::Stylize,
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

/// Something that could come next on the line.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// The whole line once this is accepted.
    pub line: String,
    pub label: String,
    pub detail: String,
}

/// A line split up for completing it: the words finished so far, and the
/// one being typed.
pub struct Typed<'l> {
    pub done: Vec<&'l str>,
    /// The word being typed, or "" after a space.
    pub partial: &'l str,
}

impl<'l> Typed<'l> {
    pub fn new(line: &'l str) -> Self {
        let words: Vec<&str> = line.split_whitespace().collect();
        let typing = !line.is_empty() && !line.ends_with(char::is_whitespace);
        let (done, partial) = match (typing, words.split_last()) {
            (true, Some((last, done))) => (done.to_vec(), *last),
            _ => (words, ""),
        };
        Self { done, partial }
    }

    /// Whether what's being typed could be the start of `word` (a lower
    /// case one: an action, a keyword).
    pub fn could_be(&self, word: &str) -> bool {
        word.starts_with(&self.partial.to_lowercase())
    }

    /// The line with `word` in place of the one being typed.
    pub fn complete(&self, word: &str) -> String {
        if self.done.is_empty() {
            format!("{word} ")
        } else {
            format!("{} {word} ", self.done.join(" "))
        }
    }

    /// `word` (shown as `label`), if what's being typed could be its start.
    pub fn keyword(&self, word: &str, label: &str, detail: &str) -> Option<Suggestion> {
        self.could_be(word).then(|| Suggestion {
            line: self.complete(word),
            label: label.into(),
            detail: detail.into(),
        })
    }

    /// Names (with their details) that may span several words, coming after
    /// the first `start` finished words: offered while what's typed from
    /// there could still be the start of one, and not once it's all typed.
    pub fn names_from(&self, start: usize, names: Vec<(String, String)>) -> Vec<Suggestion> {
        let mut typed: Vec<&str> = self.done.get(start..).unwrap_or_default().to_vec();
        if !self.partial.is_empty() {
            typed.push(self.partial);
        }
        let typed = typed.join(" ").to_lowercase();
        let head = self.done[..start.min(self.done.len())].join(" ");
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
}

/// The line being typed, and which of its suggestions is highlighted.
#[derive(Default)]
pub struct Prompt {
    pub input: String,
    pub selected: usize,
}

/// What a key asks of the screen.
pub enum Keyed {
    /// Nothing: the key edited the line or moved the highlight, if it
    /// meant anything.
    Edited,
    /// Run the line.
    Enter,
    /// Leave the screen.
    Quit,
}

impl Prompt {
    /// Handles a key, given the line's suggestions as they are now.
    pub fn key(&mut self, key: KeyEvent, suggestions: &[Suggestion]) -> Keyed {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'd') if ctrl => return Keyed::Quit,
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char('w') if ctrl => {
                let trimmed = self.input.trim_end();
                let cut = trimmed.rfind(' ').map_or(0, |i| i + 1);
                self.input.truncate(cut);
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Esc => self.input.clear(),
            KeyCode::Tab => {
                if let Some(s) = suggestions.get(self.selected) {
                    self.input = s.line.clone();
                }
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(suggestions.len().saturating_sub(1));
                return Keyed::Edited;
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                return Keyed::Edited;
            }
            KeyCode::Enter => return Keyed::Enter,
            _ => return Keyed::Edited,
        }
        // A changed line has new suggestions: back to the first.
        self.selected = 0;
        Keyed::Edited
    }

    /// Empties the line, once it has run.
    pub fn clear(&mut self) {
        self.input.clear();
        self.selected = 0;
    }
}

/// What the command box says besides the line itself.
pub struct Shown<'a> {
    /// The line in words.
    pub sentence: &'a str,
    /// What's missing or wrong, while the line can't run.
    pub problem: Option<&'a str>,
    pub suggestions: &'a [Suggestion],
    /// The outcome of the last line, and whether it was a failure: shown
    /// while the line is empty.
    pub status: Option<&'a (String, bool)>,
    /// Shown instead, with nothing to report.
    pub hint: &'a str,
    /// The keys, on the box's border.
    pub help: &'a str,
}

/// Draws the command box: the line, what it means (or the outcome of the
/// last one), and up to `rows` of its suggestions.
pub fn draw(frame: &mut Frame, area: Rect, prompt: &Prompt, shown: &Shown, rows: usize) {
    let mut lines = vec![Line::from(vec![
        Span::raw("> ").bold(),
        Span::raw(prompt.input.clone()),
    ])];
    lines.push(if prompt.input.is_empty() {
        match shown.status {
            Some((text, true)) => Line::from(text.clone()).red(),
            Some((text, false)) => Line::from(format!("✓ {text}")).green(),
            None => Line::from(shown.hint.to_string()).dark_gray(),
        }
    } else {
        match shown.problem {
            None => Line::from(vec![
                Span::raw(shown.sentence.to_string()).bold(),
                Span::raw("   ⏎").green(),
            ]),
            Some(problem) => Line::from(vec![
                Span::raw(shown.sentence.to_string()),
                Span::raw(if shown.sentence.is_empty() { "" } else { "   " }),
                Span::raw(problem.to_string()).yellow(),
            ]),
        }
    });

    let first = prompt.selected.saturating_sub(rows.saturating_sub(1));
    let width = shown
        .suggestions
        .iter()
        .map(|s| s.label.chars().count())
        .max()
        .unwrap_or(0);
    for (i, s) in shown.suggestions.iter().enumerate().skip(first).take(rows) {
        let line = Line::from(vec![
            Span::raw(format!("  {:<width$}  ", s.label)),
            Span::raw(s.detail.clone()).dark_gray(),
        ]);
        lines.push(if i == prompt.selected {
            line.reversed()
        } else {
            line
        });
    }

    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(shown.help)),
        area,
    );
    let typed = u16::try_from(prompt.input.chars().count()).unwrap_or(u16::MAX);
    let x = area.x.saturating_add(3).saturating_add(typed);
    frame.set_cursor_position((x.min(area.right().saturating_sub(2)), area.y + 1));
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyEventKind;

    use super::*;

    fn press(prompt: &mut Prompt, code: KeyCode, suggestions: &[Suggestion]) -> Keyed {
        let key = KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(key.kind, KeyEventKind::Press);
        prompt.key(key, suggestions)
    }

    fn suggestion(line: &str) -> Suggestion {
        Suggestion {
            line: line.into(),
            label: line.trim().into(),
            detail: String::new(),
        }
    }

    #[test]
    fn the_highlight_stays_on_the_suggestions_and_tab_takes_it() {
        let suggestions = [suggestion("damage "), suggestion("drop ")];
        let mut prompt = Prompt::default();
        press(&mut prompt, KeyCode::Char('d'), &suggestions);
        for _ in 0..5 {
            press(&mut prompt, KeyCode::Down, &suggestions);
        }
        assert_eq!(prompt.selected, 1);
        press(&mut prompt, KeyCode::Tab, &suggestions);
        assert_eq!(prompt.input, "drop ");
        // A changed line starts at its first suggestion again.
        assert_eq!(prompt.selected, 0);
        press(&mut prompt, KeyCode::Up, &suggestions);
        assert_eq!(prompt.selected, 0);
    }

    #[test]
    fn a_line_splits_into_what_is_done_and_what_is_being_typed() {
        let typed = Typed::new("cast hunter");
        assert_eq!(
            (typed.done.as_slice(), typed.partial),
            (&["cast"][..], "hunter")
        );
        assert_eq!(Typed::new("cast hunter ").partial, "", "after a space");
        assert_eq!(typed.complete("Healing"), "cast Healing ");
        assert!(typed.keyword("for", "for <rounds>", "").is_none());
        assert!(
            Typed::new("effect blessed f")
                .keyword("for", "for", "")
                .is_some()
        );
        let spells = || {
            vec![
                ("Hunter's Mark".to_string(), String::new()),
                ("Healing Word".to_string(), String::new()),
            ]
        };
        let lines = |line: &str, start| -> Vec<String> {
            Typed::new(line)
                .names_from(start, spells())
                .into_iter()
                .map(|s| s.line)
                .collect()
        };
        assert_eq!(lines("cast hunter", 1), ["cast Hunter's Mark "]);
        // Names span words: "hunter's m" is still Hunter's Mark.
        assert_eq!(lines("cast hunter's m", 1), ["cast Hunter's Mark "]);
        assert_eq!(lines("cast ", 1).len(), 2);
        // Once a whole name is typed, it's not offered again.
        assert!(lines("cast hunter's mark ", 1).is_empty());
        // After a number and an action.
        assert_eq!(lines("2 cast he", 2), ["2 cast Healing Word "]);
    }

    #[test]
    fn keys_edit_the_line_and_ask_to_run_or_leave() {
        let mut prompt = Prompt {
            input: "cast healing word".into(),
            selected: 0,
        };
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        prompt.key(ctrl('w'), &[]);
        assert_eq!(prompt.input, "cast healing ");
        press(&mut prompt, KeyCode::Backspace, &[]);
        assert_eq!(prompt.input, "cast healing");
        assert!(matches!(
            press(&mut prompt, KeyCode::Enter, &[]),
            Keyed::Enter
        ));
        press(&mut prompt, KeyCode::Esc, &[]);
        assert_eq!(prompt.input, "");
        assert!(matches!(prompt.key(ctrl('c'), &[]), Keyed::Quit));
    }
}
