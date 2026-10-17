//! Styled, wrapped text for character sheets and stat blocks. It's built
//! once as ratatui lines: the terminal screens draw them as they are, and
//! printing turns them into terminal colours, or plain text when piped.
//!
//! Sections open with a rule (`── Inventory ───`), list entries with a
//! bullet, labels are dim beside their values, and wrapped lines keep their
//! indent, so a long entry never looks like the start of a new one.

use std::{
    cell::Cell,
    hash::{DefaultHasher, Hash, Hasher},
    io::IsTerminal,
};

use ratatui::{
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
};

/// A space that lines aren't broken at, for text that belongs together
/// ("Passive Perception 15"); printed as an ordinary space.
pub const KEEP: char = '\u{a0}';

/// `text` with its spaces made ones lines aren't broken at.
pub fn together(text: &str) -> String {
    text.replace(' ', &KEEP.to_string())
}

/// Labels and secondary detail.
pub fn dim(text: impl Into<String>) -> Span<'static> {
    Span::raw(text.into()).dark_gray()
}

pub fn plain(text: impl Into<String>) -> Span<'static> {
    Span::raw(text.into())
}

pub fn strong(text: impl Into<String>) -> Span<'static> {
    Span::raw(text.into()).bold()
}

/// Lines wrapped to a width, built up section by section.
pub struct Doc {
    width: usize,
    lines: Vec<Line<'static>>,
}

impl Doc {
    pub fn new(width: usize) -> Self {
        Self {
            width: width.max(20),
            lines: Vec::new(),
        }
    }

    pub fn into_lines(mut self) -> Vec<Line<'static>> {
        while self.lines.last().is_some_and(|l| l.width() == 0) {
            self.lines.pop();
        }
        self.lines
    }

    pub fn blank(&mut self) {
        self.lines.push(Line::default());
    }

    /// A line of its own; wrapped lines are indented by two.
    pub fn line(&mut self, content: Vec<Span<'static>>) {
        self.push(Vec::new(), 2, content);
    }

    /// Opens a section: a blank line, then the name in a rule across.
    pub fn heading(&mut self, name: &str) {
        if !self.lines.is_empty() {
            self.blank();
        }
        let rule = self.width.saturating_sub(name.chars().count() + 4).max(2);
        self.lines.push(Line::from(vec![
            dim("── "),
            Span::raw(name.to_string()).cyan().bold(),
            dim(format!(" {}", "─".repeat(rule))),
        ]));
    }

    /// A list entry: a bullet, and wrapped lines under its text.
    pub fn bullet(&mut self, content: Vec<Span<'static>>) {
        self.push(vec![dim("  • ")], 4, content);
    }

    /// Text under a bullet's, such as an entry's later paragraphs.
    pub fn under_bullet(&mut self, content: Vec<Span<'static>>) {
        self.push(vec![plain("    ")], 4, content);
    }

    /// A paragraph in a section.
    pub fn paragraph(&mut self, content: Vec<Span<'static>>) {
        self.push(vec![plain("  ")], 2, content);
    }

    /// A dim label in a column `width` wide, with its value beside it and
    /// wrapped lines under the value. `indent` is where the label starts.
    pub fn row(&mut self, indent: usize, label: &str, width: usize, value: Vec<Span<'static>>) {
        let first = vec![plain(" ".repeat(indent)), dim(format!("{label:<width$}  "))];
        self.push(first, indent + width + 2, value);
    }

    /// Wraps `content` after `first`, starting later lines at `indent`.
    fn push(&mut self, first: Vec<Span<'static>>, indent: usize, content: Vec<Span<'static>>) {
        self.lines.extend(wrap(first, indent, &content, self.width));
    }
}

/// How far to scroll `lines` in a box `height` rows tall to bring the
/// section `heading` to the top: not at all if everything fits or there's
/// no such section, and never so far that the end leaves rows empty.
pub fn scroll_to(lines: &[Line<'static>], heading: Option<&str>, height: usize) -> usize {
    let Some(heading) = heading else { return 0 };
    if lines.len() <= height {
        return 0;
    }
    lines
        .iter()
        .position(|line| {
            // As `Doc::heading` lays it out: the rule's start, then the name.
            line.spans.len() == 3
                && line.spans[0].content == "── "
                && line.spans[1].content == heading
        })
        .map_or(0, |at| at.min(lines.len() - height))
}

/// A panel scrolled by hand (Alt-↑/↓), on top of wherever it would be
/// anyway. What was scrolled is forgotten when the panel shows something
/// else. Kept in cells, because only drawing knows how far it can go.
#[derive(Debug, Default)]
pub struct PanelScroll {
    /// Lines scrolled by hand: down if positive.
    by_hand: Cell<isize>,
    /// What the panel showed when last drawn, as a hash.
    showing: Cell<u64>,
    /// When last drawn: where it would have been, and how far it can go.
    bounds: Cell<(usize, usize)>,
}

/// What a scrolled panel shows: from which line, and how many are hidden
/// above and below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scrolled {
    pub offset: usize,
    pub above: usize,
    pub below: usize,
}

impl PanelScroll {
    /// Scrolls by `lines`, as far as the last drawing allows.
    pub fn by(&self, lines: isize) {
        let (automatic, most) = self.bounds.get();
        let wanted = automatic.saturating_add_signed(self.by_hand.get() + lines);
        let at = wanted.min(most);
        self.by_hand.set(at as isize - automatic as isize);
    }

    /// Where to draw `lines` lines in `height` rows, given where they'd be
    /// without scrolling by hand and what they show.
    pub fn place(
        &self,
        showing: impl Hash,
        automatic: usize,
        lines: usize,
        height: usize,
    ) -> Scrolled {
        let mut hasher = DefaultHasher::new();
        showing.hash(&mut hasher);
        let showing = hasher.finish();
        if self.showing.replace(showing) != showing {
            self.by_hand.set(0);
        }
        let most = lines.saturating_sub(height);
        let automatic = automatic.min(most);
        self.bounds.set((automatic, most));
        let offset = automatic
            .saturating_add_signed(self.by_hand.get())
            .min(most);
        Scrolled {
            offset,
            above: offset,
            below: most - offset,
        }
    }
}

impl Scrolled {
    /// For the panel's bottom border: what's hidden, and how to see it.
    pub fn hint(&self) -> Option<String> {
        let hidden = match (self.above, self.below) {
            (0, 0) => return None,
            (0, below) => format!("↓ {below} more"),
            (above, 0) => format!("↑ {above} more"),
            (above, below) => format!("↑ {above} · ↓ {below} more"),
        };
        Some(format!(" {hidden} · Alt-↑↓ scrolls "))
    }
}

/// A run of text without spaces, perhaps in more than one style ("(C),").
type Word = Vec<(String, Style)>;

enum Piece {
    Space(String, Style),
    Word(Word),
}

fn pieces(content: &[Span<'static>]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut word = Word::new();
    for span in content {
        let mut rest = span.content.as_ref();
        while let Some(c) = rest.chars().next() {
            let space = c == ' ';
            let end = rest
                .find(|c: char| (c == ' ') != space)
                .unwrap_or(rest.len());
            let (run, after) = rest.split_at(end);
            if space {
                if !word.is_empty() {
                    pieces.push(Piece::Word(std::mem::take(&mut word)));
                }
                pieces.push(Piece::Space(run.to_string(), span.style));
            } else {
                word.push((run.to_string(), span.style));
            }
            rest = after;
        }
    }
    if !word.is_empty() {
        pieces.push(Piece::Word(word));
    }
    pieces
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn wrap(
    first: Vec<Span<'static>>,
    indent: usize,
    content: &[Span<'static>],
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut start = first.iter().map(|s| chars(&s.content)).sum::<usize>();
    let mut line = first;
    let mut used = start;
    let mut spaces: Vec<(String, Style)> = Vec::new();
    for piece in pieces(content) {
        let word = match piece {
            Piece::Space(run, style) => {
                spaces.push((run, style));
                continue;
            }
            Piece::Word(word) => word,
        };
        let length: usize = word.iter().map(|(t, _)| chars(t)).sum();
        let gap: usize = spaces.iter().map(|(t, _)| chars(t)).sum();
        if used > start && used + gap + length > width {
            lines.push(Line::from(std::mem::take(&mut line)));
            line.push(plain(" ".repeat(indent)));
            start = indent;
            used = indent;
            spaces.clear();
        }
        for (text, style) in spaces.drain(..).chain(word) {
            used += chars(&text);
            line.push(Span::styled(text.replace(KEEP, " "), style));
        }
    }
    lines.push(Line::from(
        line.into_iter()
            .map(|span| Span::styled(span.content.replace(KEEP, " "), span.style))
            .collect::<Vec<_>>(),
    ));
    lines
}

/// How wide printed text may be: the terminal's width, up to 100, or 80
/// when not printing to a terminal.
pub fn print_width() -> usize {
    let terminal = std::io::stdout().is_terminal();
    match crossterm::terminal::size() {
        Ok((columns, _)) if terminal => usize::from(columns).min(100),
        _ => 80,
    }
}

/// Prints lines, in colour on a terminal (unless `NO_COLOR` is set).
pub fn print(lines: &[Line<'static>]) {
    let colour = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    println!("{}", render(lines, colour));
}

/// The lines as text, with ANSI colours or without.
pub fn render(lines: &[Line<'static>], colour: bool) -> String {
    lines
        .iter()
        .map(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|span| match ansi(span.style).filter(|_| colour) {
                    Some(codes) => format!("\x1b[{codes}m{}\x1b[0m", span.content),
                    None => span.content.to_string(),
                })
                .collect();
            text.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The ANSI codes for a style, if it has any.
fn ansi(style: Style) -> Option<String> {
    let mut codes = Vec::new();
    for (modifier, code) in [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
    ] {
        if style.add_modifier.contains(modifier) {
            codes.push(code.to_string());
        }
    }
    let colour = match style.fg {
        Some(Color::Black) => Some(30),
        Some(Color::Red) => Some(31),
        Some(Color::Green) => Some(32),
        Some(Color::Yellow) => Some(33),
        Some(Color::Blue) => Some(34),
        Some(Color::Magenta) => Some(35),
        Some(Color::Cyan) => Some(36),
        Some(Color::Gray) => Some(37),
        Some(Color::DarkGray) => Some(90),
        Some(Color::White) => Some(97),
        _ => None,
    };
    codes.extend(colour.map(|c| c.to_string()));
    (!codes.is_empty()).then(|| codes.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(doc: Doc) -> String {
        render(&doc.into_lines(), false)
    }

    #[test]
    fn wrapped_entries_keep_their_indent() {
        let mut doc = Doc::new(30);
        doc.bullet(vec![
            plain("Cloak of Elvenkind"),
            dim(" — "),
            plain("advantage on stealth checks while the hood is up"),
        ]);
        assert_eq!(
            text(doc),
            "  • Cloak of Elvenkind —\n    advantage on stealth\n    checks while the hood is\n    up"
        );
    }

    #[test]
    fn rows_wrap_under_their_value() {
        let mut doc = Doc::new(30);
        doc.row(
            2,
            "1st",
            8,
            vec![plain("Hunter's Mark, Fog Cloud, Goodberry")],
        );
        assert_eq!(
            text(doc),
            "  1st       Hunter's Mark, Fog\n            Cloud, Goodberry"
        );
    }

    #[test]
    fn headings_are_rules_across_the_width() {
        let mut doc = Doc::new(24);
        doc.line(vec![strong("Vex")]);
        doc.heading("Inventory");
        assert_eq!(text(doc), "Vex\n\n── Inventory ───────────");
    }

    #[test]
    fn colour_is_only_added_when_wanted() {
        let lines = vec![Line::from(vec![dim("HP "), strong("38/44")])];
        assert_eq!(render(&lines, false), "HP 38/44");
        assert_eq!(
            render(&lines, true),
            "\x1b[90mHP \x1b[0m\x1b[1m38/44\x1b[0m"
        );
    }

    #[test]
    fn scrolling_brings_a_section_up_only_when_needed() {
        let mut doc = Doc::new(30);
        for n in 0..5 {
            doc.line(vec![plain(format!("line {n}"))]);
        }
        doc.heading("Spellcasting");
        doc.paragraph(vec![plain("Hex")]);
        doc.paragraph(vec![plain("Healing Word")]);
        let lines = doc.into_lines();
        // Five lines, a blank, the heading at 6, and two spells: 9.
        assert_eq!(scroll_to(&lines, Some("Spellcasting"), 9), 0, "it all fits");
        assert_eq!(scroll_to(&lines, Some("Spellcasting"), 3), 6);
        assert_eq!(
            scroll_to(&lines, Some("Spellcasting"), 5),
            4,
            "the end stays at the bottom"
        );
        assert_eq!(scroll_to(&lines, Some("Features"), 3), 0, "no such section");
        assert_eq!(scroll_to(&lines, None, 3), 0);
    }

    #[test]
    fn panels_scroll_by_hand_within_bounds_until_they_show_something_else() {
        let scroll = PanelScroll::default();
        // 20 lines in 5 rows, automatically at line 6.
        let at = scroll.place("Vex", 6, 20, 5);
        assert_eq!((at.offset, at.above, at.below), (6, 6, 9));
        scroll.by(-2);
        assert_eq!(scroll.place("Vex", 6, 20, 5).offset, 4);
        scroll.by(100);
        assert_eq!(
            scroll.place("Vex", 6, 20, 5).offset,
            15,
            "no further than the end"
        );
        scroll.by(-100);
        assert_eq!(scroll.place("Vex", 6, 20, 5).offset, 0, "nor the start");
        scroll.by(3);
        assert_eq!(scroll.place("Grog", 0, 20, 5).offset, 0, "forgotten");
        assert_eq!(scroll.place("Grog", 0, 4, 5).hint(), None, "it all fits");
        assert_eq!(
            scroll.place("Grog", 0, 20, 5).hint().unwrap(),
            " ↓ 15 more · Alt-↑↓ scrolls "
        );
    }

    #[test]
    fn a_word_too_long_for_the_line_still_appears() {
        let mut doc = Doc::new(20);
        doc.paragraph(vec![plain("a supercalifragilisticexpialidocious b")]);
        assert_eq!(text(doc), "  a\n  supercalifragilisticexpialidocious\n  b");
    }
}
