//! The encounter planner: a terminal screen for putting together fights
//! ahead of time, from the campaign's monsters, and seeing how hard they'd
//! be for the party. Plans are kept in the campaign; `fight` (here, or
//! `campaign-edit fight --plan`) starts a fresh combat from one.

pub mod command;
mod ui;

use std::collections::HashMap;

use anyhow::Result;
use campaign_types::{
    CharacterSummary,
    encounter::{Encounter, EncounterPlan},
    monster::{Monster, MonsterSummary},
};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use uuid::Uuid;

use crate::fight::{
    Login,
    command::{Suggestion, monster_named},
};
use command::{Context, Parsed, Step};

pub struct App {
    login: Login,
    /// The campaign's plans, by name.
    encounters: Vec<Encounter>,
    /// The plan being worked on.
    current: Option<Uuid>,
    bestiary: Vec<MonsterSummary>,
    roster: Vec<CharacterSummary>,
    /// Stat blocks fetched so far, by monster id.
    stat_blocks: HashMap<Uuid, Monster>,
    input: String,
    selected: usize,
    /// The outcome of the last line, and whether it was a failure.
    status: Option<(String, bool)>,
}

/// Opens the planner on the plan `wanted` (a new one, if there's none by
/// that name) or the most recently changed one. Returns the plan to start
/// a fight from, if that's how it was left.
pub async fn run(
    wanted: Option<&str>,
    characters_url: &str,
    combat_url: &str,
    monsters_url: &str,
) -> Result<Option<Uuid>> {
    let mut login = Login::new(characters_url, combat_url, monsters_url).await?;
    let combats = login.combats().await?;
    let mut encounters = combats.encounters().await?;
    let current = match wanted {
        Some(name) => match command::find_plan(&encounters, name, false) {
            Some(found) => Some(found.id),
            None => {
                let plan = EncounterPlan {
                    name: name.to_string(),
                    ..Default::default()
                };
                let created = combats.create_encounter(&plan).await?;
                let id = created.id;
                encounters.push(created);
                Some(id)
            }
        },
        None => encounters.iter().max_by_key(|e| e.updated_at).map(|e| e.id),
    };
    let roster = login.characters().await?.list().await?;
    let bestiary = login.monsters().await?.list(None).await?;
    let mut app = App {
        login,
        encounters,
        current,
        bestiary,
        roster,
        stat_blocks: HashMap::new(),
        input: String::new(),
        selected: 0,
        status: None,
    };
    app.sort();

    let mut terminal = ratatui::init();
    let mut keys = EventStream::new();
    let result = loop {
        if let Err(e) = terminal.draw(|frame| ui::draw(frame, &app)) {
            break Err(e.into());
        }
        match keys.next().await {
            Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                match app.key(key).await {
                    Leave::Stay => app.fetch_shown().await,
                    Leave::Quit => break Ok(None),
                    Leave::Fight(id) => break Ok(Some(id)),
                }
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => break Err(e.into()),
            None => break Ok(None),
        }
    };
    ratatui::restore();
    result
}

enum Leave {
    Stay,
    Quit,
    Fight(Uuid),
}

impl App {
    pub fn context(&self) -> Context<'_> {
        Context {
            encounters: &self.encounters,
            current: self.current(),
            bestiary: &self.bestiary,
            roster: &self.roster,
        }
    }

    pub fn current(&self) -> Option<&Encounter> {
        let id = self.current?;
        self.encounters.iter().find(|e| e.id == id)
    }

    pub fn parsed(&self) -> Parsed {
        command::parse(&self.input, &self.context())
    }

    pub fn suggestions(&self) -> Vec<Suggestion> {
        command::suggest(&self.input, &self.context())
    }

    /// The monster whose stat block to show: the one the line is about,
    /// or the one the highlighted suggestion names.
    pub fn shown(&self) -> Option<Uuid> {
        self.parsed().monster.or_else(|| {
            let suggestion = self.suggestions().into_iter().nth(self.selected)?;
            monster_named(&self.bestiary, &suggestion.label).map(|m| m.id)
        })
    }

    pub fn summary(&self, id: Uuid) -> Option<&MonsterSummary> {
        self.bestiary.iter().find(|m| m.id == id)
    }

    fn sort(&mut self) {
        self.encounters.sort_by_key(|e| e.plan.name.to_lowercase());
    }

    async fn fetch_shown(&mut self) {
        let Some(id) = self.shown() else { return };
        if self.stat_blocks.contains_key(&id) {
            return;
        }
        let fetched = match self.login.monsters().await {
            Ok(client) => client.get(id).await.map_err(Into::into),
            Err(e) => Err(e),
        };
        match fetched {
            Ok(monster) => {
                self.stat_blocks.insert(id, monster);
            }
            Err(e) => self.status = Some((format!("couldn't load the stat block: {e:#}"), true)),
        }
    }

    async fn key(&mut self, key: KeyEvent) -> Leave {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c' | 'd') if ctrl => return Leave::Quit,
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
                if let Some(s) = self.suggestions().get(self.selected) {
                    self.input = s.line.clone();
                }
            }
            KeyCode::Down => self.selected += 1,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter => return self.enter().await,
            _ => return Leave::Stay,
        }
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.selected = 0;
        }
        let count = self.suggestions().len();
        self.selected = self.selected.min(count.saturating_sub(1));
        Leave::Stay
    }

    async fn enter(&mut self) -> Leave {
        let parsed = self.parsed();
        let step = match parsed.plan {
            Ok(step) => step,
            Err(problem) => {
                self.status = Some((problem, true));
                return Leave::Stay;
            }
        };
        let outcome = match step {
            Step::Quit => return Leave::Quit,
            Step::Fight => match self.current {
                Some(id) => return Leave::Fight(id),
                None => return Leave::Stay,
            },
            step => self.change(step).await,
        };
        self.status = Some(match outcome {
            Ok(()) => (parsed.sentence, false),
            Err(e) => (format!("{}: {e:#}", parsed.sentence), true),
        });
        self.input.clear();
        self.selected = 0;
        Leave::Stay
    }

    /// Makes a change to the campaign's plans.
    async fn change(&mut self, step: Step) -> Result<()> {
        let combats = self.login.combats().await?;
        match step {
            Step::New(name) => {
                let plan = EncounterPlan {
                    name,
                    ..Default::default()
                };
                let created = combats.create_encounter(&plan).await?;
                self.current = Some(created.id);
                self.encounters.push(created);
            }
            Step::Open(id) => self.current = Some(id),
            Step::Save(plan) => {
                let Some(id) = self.current else {
                    return Ok(());
                };
                let saved = combats.replace_encounter(id, &plan).await?;
                if let Some(old) = self.encounters.iter_mut().find(|e| e.id == id) {
                    *old = saved;
                }
            }
            Step::Delete(id) => {
                combats.delete_encounter(id).await?;
                self.encounters.retain(|e| e.id != id);
                if self.current == Some(id) {
                    self.current = None;
                }
            }
            Step::Fight | Step::Quit => {}
        }
        self.sort();
        Ok(())
    }
}
