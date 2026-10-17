//! The encounter planner: a terminal screen for putting together fights
//! ahead of time, from the campaign's monsters, and seeing how hard they'd
//! be for the party. Plans are kept in the campaign; `fight` (here, or
//! `edda fight --plan`) starts a fresh combat from one.

pub mod command;
mod ui;

use std::collections::HashMap;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use edda_types::{
    CharacterSummary, Edition,
    encounter::{Encounter, EncounterPlan},
    monster::{Monster, MonsterSummary},
};
use futures_util::StreamExt;
use uuid::Uuid;

use crate::{
    prompt::{Keyed, Prompt, Suggestion},
    session::{self, Login},
};
use command::{Context, Parsed, Step};

/// Everything the planner shows, and what it makes of the line being
/// typed: no connections, so it can be drawn (and tested) alone.
pub struct Screen {
    /// The campaign's plans, by name.
    pub(crate) encounters: Vec<Encounter>,
    /// The plan being worked on.
    pub(crate) current: Option<Uuid>,
    pub(crate) bestiary: Vec<MonsterSummary>,
    pub(crate) roster: Vec<CharacterSummary>,
    /// Stat blocks fetched so far, by monster id.
    pub(crate) stat_blocks: HashMap<Uuid, Monster>,
    pub(crate) prompt: Prompt,
    /// The outcome of the last line, and whether it was a failure.
    pub(crate) status: Option<(String, bool)>,
    /// The stat block panel, scrolled by hand.
    pub(crate) panel: crate::styled::PanelScroll,
    /// This device's name, and the rules the campaign plays by.
    pub(crate) me: String,
    pub(crate) edition: Edition,
}

/// The planner, connected.
pub struct App {
    screen: Screen,
    login: Login,
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
        Some(name) => {
            if let Some(found) = session::find_plan(&encounters, name, false) {
                Some(found.id)
            } else {
                let plan = EncounterPlan {
                    name: name.to_string(),
                    ..Default::default()
                };
                let created = combats.create_encounter(&plan).await?;
                let id = created.id;
                encounters.push(created);
                Some(id)
            }
        }
        None => encounters.iter().max_by_key(|e| e.updated_at).map(|e| e.id),
    };
    let roster = login.characters().await?.list().await?;
    let bestiary = login.monsters().await?.list(None).await?;
    let mut app = App {
        screen: Screen {
            encounters,
            current,
            bestiary,
            roster,
            stat_blocks: HashMap::new(),
            prompt: Prompt::default(),
            status: None,
            panel: crate::styled::PanelScroll::default(),
            me: login.session.claims.name.clone(),
            edition: login.profile.campaign.edition,
        },
        login,
    };
    app.screen.sort();

    let mut terminal = ratatui::init();
    let mut keys = EventStream::new();
    let result = loop {
        if let Err(e) = terminal.draw(|frame| ui::draw(frame, &app.screen)) {
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

impl Screen {
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
        command::parse(&self.prompt.input, &self.context())
    }

    pub fn suggestions(&self) -> Vec<Suggestion> {
        command::suggest(&self.prompt.input, &self.context())
    }

    /// The monster whose stat block to show: the one the line is about,
    /// or the one the highlighted suggestion names.
    pub fn shown(&self) -> Option<Uuid> {
        self.parsed().monster.or_else(|| {
            let suggestion = self.suggestions().into_iter().nth(self.prompt.selected)?;
            session::monster_named(&self.bestiary, &suggestion.label).map(|m| m.id)
        })
    }

    pub fn summary(&self, id: Uuid) -> Option<&MonsterSummary> {
        self.bestiary.iter().find(|m| m.id == id)
    }

    fn sort(&mut self) {
        self.encounters.sort_by_key(|e| e.plan.name.to_lowercase());
    }
}

impl App {
    async fn fetch_shown(&mut self) {
        let Some(id) = self.screen.shown() else {
            return;
        };
        if self.screen.stat_blocks.contains_key(&id) {
            return;
        }
        let fetched = match self.login.monsters().await {
            Ok(client) => client.get(id).await.map_err(Into::into),
            Err(e) => Err(e),
        };
        match fetched {
            Ok(monster) => {
                self.screen.stat_blocks.insert(id, monster);
            }
            Err(e) => {
                self.screen.status = Some((format!("couldn't load the stat block: {e:#}"), true));
            }
        }
    }

    async fn key(&mut self, key: KeyEvent) -> Leave {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Up if alt => self.screen.panel.by(-1),
            KeyCode::Down if alt => self.screen.panel.by(1),
            _ => match self.screen.prompt.key(key, &self.screen.suggestions()) {
                Keyed::Edited => {}
                Keyed::Enter => return self.enter().await,
                Keyed::Quit => return Leave::Quit,
            },
        }
        Leave::Stay
    }

    async fn enter(&mut self) -> Leave {
        let parsed = self.screen.parsed();
        let step = match parsed.plan {
            Ok(step) => step,
            Err(problem) => {
                self.screen.status = Some((problem, true));
                return Leave::Stay;
            }
        };
        let outcome = match step {
            Step::Quit => return Leave::Quit,
            Step::Fight => match self.screen.current {
                Some(id) => return Leave::Fight(id),
                None => return Leave::Stay,
            },
            step => self.change(step).await,
        };
        self.screen.status = Some(match outcome {
            Ok(()) => (parsed.sentence, false),
            Err(e) => (format!("{}: {e:#}", parsed.sentence), true),
        });
        self.screen.prompt.clear();
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
                self.screen.current = Some(created.id);
                self.screen.encounters.push(created);
            }
            Step::Open(id) => self.screen.current = Some(id),
            Step::Save(plan) => {
                let Some(id) = self.screen.current else {
                    return Ok(());
                };
                let saved = combats.replace_encounter(id, &plan).await?;
                if let Some(old) = self.screen.encounters.iter_mut().find(|e| e.id == id) {
                    *old = saved;
                }
            }
            Step::Delete(id) => {
                combats.delete_encounter(id).await?;
                self.screen.encounters.retain(|e| e.id != id);
                if self.screen.current == Some(id) {
                    self.screen.current = None;
                }
            }
            Step::Fight | Step::Quit => {}
        }
        self.screen.sort();
        Ok(())
    }
}
