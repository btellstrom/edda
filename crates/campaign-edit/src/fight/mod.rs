//! A terminal screen for running a combat: the initiative order, the sheet
//! of whoever is acting, the log, and a command line that completes
//! actions, spells and combatants and says who does what to whom before
//! anything is sent.

pub mod command;
mod log;
mod ui;

use std::{collections::HashMap, time::Duration};

use anyhow::{Context as _, Result, anyhow, bail};
use campaign_auth::protocol::Principal;
use campaign_client::combat::{Received, Replica, live::CommandSender};
use campaign_types::{
    Character, CharacterSummary,
    combat::{
        CombatCommand, CombatEvent, CombatStatus, CombatSummary, Combatant, CommandEnvelope,
        LoggedEvent, ServerMessage,
    },
    monster::{Monster, MonsterSummary},
    rules_text::{ConditionEntry, ConditionText, SpellEntry, SpellText},
};
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    cli::FightArgs,
    prompt::{Keyed, Prompt, Suggestion},
    session::{self, Login},
};
use command::{Context, Parsed, Step, Topic};

/// What the WebSocket reader passes to the screen.
enum Incoming {
    Message(ServerMessage),
    Closed(String),
    /// Time to try reconnecting again.
    Retry,
    /// Time to read this character's sheet again.
    Sheet(Uuid),
}

pub struct App {
    login: Login,
    combat: Uuid,
    replica: Replica,
    log: Vec<LoggedEvent>,
    /// Names of every combatant and effect ever in the combat, for the log.
    names: HashMap<Uuid, String>,
    sheets: HashMap<Uuid, Character>,
    roster: Vec<CharacterSummary>,
    /// The monsters that can be added.
    bestiary: Vec<MonsterSummary>,
    /// Stat blocks of the monsters in the fight, by monster id.
    monsters: HashMap<Uuid, Monster>,
    /// The spells' and conditions' descriptions.
    spells: Vec<SpellEntry>,
    conditions: Vec<ConditionEntry>,
    /// What `show` keeps in the side panel, until Esc or another line.
    pinned: Option<Topic>,
    sender: Option<CommandSender>,
    incoming: mpsc::UnboundedSender<Incoming>,
    prompt: Prompt,
    /// The outcome of the last line, and whether it was a failure.
    status: Option<(String, bool)>,
    /// How many lines the log is scrolled back.
    scroll: usize,
    /// The sheet panel, scrolled by hand.
    panel: crate::styled::PanelScroll,
    /// Failed reconnection attempts in a row.
    retries: u32,
    /// This device's key, if it's a player's.
    player: Option<String>,
}

pub async fn run(args: FightArgs, characters_url: &str, monsters_url: &str) -> Result<()> {
    let mut login = Login::new(characters_url, &args.combat_server, monsters_url).await?;
    let combats = login.combats().await?;
    let combat = match (&args.plan, &args.new) {
        (Some(wanted), _) => {
            let encounters = combats.encounters().await?;
            let found = session::find_plan(&encounters, wanted, false).ok_or_else(|| {
                anyhow!("no plan matches \"{wanted}\" (see `campaign-edit plan`)")
            })?;
            // The combat service puts the plan's monsters in it.
            combats.create_from_plan(found.id, None).await?.id
        }
        (None, Some(name)) => combats.create(name).await?.id,
        (None, None) => choose(&combats.list(None).await?, args.combat.as_deref())?,
    };
    let log = combats.log(combat, 0).await?;
    let roster = login.characters().await?.list().await?;
    // The fight works without monsters, so their service being away
    // isn't fatal.
    let monster_client = login.monsters().await?;
    let (bestiary, monster_problem) = match monster_client.list(None).await {
        Ok(bestiary) => (bestiary, None),
        Err(e) => (Vec::new(), Some(format!("no monsters to add: {e}"))),
    };
    // Nor are the descriptions.
    let spells = monster_client
        .texts::<SpellText>(None)
        .await
        .unwrap_or_default();
    let conditions = monster_client
        .texts::<ConditionText>(None)
        .await
        .unwrap_or_default();

    let (incoming, mut received) = mpsc::unbounded_channel();
    let (sender, snapshot) = connect(&mut login, combat, &incoming).await?;
    let claims = &login.session.claims;
    let player = (claims.role == Principal::Player).then(|| claims.key.to_string());
    let mut app = App {
        login,
        combat,
        replica: Replica::new(snapshot),
        names: log::names(&log),
        log,
        sheets: HashMap::new(),
        roster,
        bestiary,
        monsters: HashMap::new(),
        spells,
        conditions,
        pinned: None,
        sender: Some(sender),
        incoming,
        prompt: Prompt::default(),
        status: monster_problem.map(|problem| (problem, true)),
        scroll: 0,
        panel: Default::default(),
        retries: 0,
        player,
    };
    app.refresh_sheets().await;

    let mut terminal = ratatui::init();
    let mut keys = EventStream::new();
    let result = loop {
        if let Err(e) = terminal.draw(|frame| ui::draw(frame, &app)) {
            break Err(e.into());
        }
        tokio::select! {
            key = keys.next() => match key {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    if !app.key(key).await {
                        break Ok(());
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => break Err(e.into()),
                None => break Ok(()),
            },
            Some(message) = received.recv() => app.receive(message).await,
        }
    };
    ratatui::restore();
    result
}

/// The combat named (by name or id prefix) on the command line, or the
/// most recently active one that hasn't ended.
fn choose(combats: &[CombatSummary], wanted: Option<&str>) -> Result<Uuid> {
    let Some(wanted) = wanted else {
        return combats
            .iter()
            .find(|c| c.status != CombatStatus::Ended)
            .map(|c| c.id)
            .ok_or_else(|| anyhow!("no combat in progress; start one with --new <NAME>"));
    };
    let wanted = wanted.to_lowercase();
    let matching: Vec<_> = combats
        .iter()
        .filter(|c| {
            c.name.to_lowercase().starts_with(&wanted) || c.id.to_string().starts_with(&wanted)
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(one.id),
        [] => bail!("no combat matches \"{wanted}\""),
        many => {
            let exact: Vec<_> = many
                .iter()
                .filter(|c| c.name.eq_ignore_ascii_case(&wanted))
                .collect();
            match exact.as_slice() {
                [one] => Ok(one.id),
                _ => {
                    let names: Vec<_> = many.iter().map(|c| c.name.as_str()).collect();
                    bail!("\"{wanted}\" could be {}", names.join(", "))
                }
            }
        }
    }
}

/// Opens the live connection: a reader task forwarding everything to
/// `incoming`, the sender, and the first snapshot.
async fn connect(
    login: &mut Login,
    combat: Uuid,
    incoming: &mpsc::UnboundedSender<Incoming>,
) -> Result<(CommandSender, campaign_types::combat::CombatSnapshot)> {
    let connection = login.combats().await?.connect(combat).await?;
    let (sender, mut receiver) = connection.split();
    let snapshot = match receiver.recv().await {
        Some(Ok(ServerMessage::Snapshot(snapshot))) => snapshot,
        Some(Err(e)) => return Err(e).context("connecting to the combat"),
        _ => bail!("the combat service didn't send the combat"),
    };
    let incoming = incoming.clone();
    tokio::spawn(async move {
        let reason = loop {
            match receiver.recv().await {
                Some(Ok(message)) => {
                    if incoming.send(Incoming::Message(message)).is_err() {
                        return;
                    }
                }
                Some(Err(e)) => break e.to_string(),
                None => break "the connection closed".to_string(),
            }
        };
        let _ = incoming.send(Incoming::Closed(reason));
    });
    Ok((sender, snapshot))
}

impl App {
    pub fn context(&self) -> Context<'_> {
        Context {
            state: self.replica.state(),
            sheets: &self.sheets,
            roster: &self.roster,
            bestiary: &self.bestiary,
            spells: &self.spells,
            conditions: &self.conditions,
            player: self.player.as_deref(),
        }
    }

    /// What the side panel describes instead of the acting combatant's
    /// sheet: what the line is about (the suggestion picked, while its name
    /// is being typed: "cast heal" is about Healing Word, not Heal), else
    /// what `show` keeps there.
    pub fn topic(&self) -> Option<Topic> {
        let ctx = self.context();
        if let Some(typed) = self.parsed().topic {
            let picked = self
                .suggestions()
                .into_iter()
                .nth(self.prompt.selected)
                .map(|s| match &typed {
                    Topic::Spell(_) => Topic::Spell(s.label),
                    Topic::Effect(_) => Topic::Effect(s.label),
                    _ => Topic::Named(s.label),
                });
            if let Some(topic) = picked.into_iter().chain([typed]).find(|t| ctx.describes(t)) {
                return Some(topic);
            }
        }
        self.pinned.clone().filter(|t| ctx.describes(t))
    }

    pub fn parsed(&self) -> Parsed {
        command::parse(&self.prompt.input, &self.context())
    }

    pub fn suggestions(&self) -> Vec<Suggestion> {
        command::suggest(&self.prompt.input, &self.context())
    }

    /// The combatant whose sheet to show: whoever the line is about.
    pub fn shown(&self) -> Option<&Combatant> {
        let id = self.parsed().actor?;
        self.replica.state().combatant(id)
    }

    /// Handles a key; `false` to leave.
    async fn key(&mut self, key: KeyEvent) -> bool {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Up if alt => self.panel.by(-1),
            KeyCode::Down if alt => self.panel.by(1),
            KeyCode::PageUp => self.scroll += 5,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(5),
            // A second Esc, on an empty line, lets go of what `show` kept.
            KeyCode::Esc if self.prompt.input.is_empty() => self.pinned = None,
            _ => match self.prompt.key(key, &self.suggestions()) {
                Keyed::Edited => {}
                Keyed::Enter => return self.enter().await,
                Keyed::Quit => return false,
            },
        }
        true
    }

    async fn enter(&mut self) -> bool {
        let parsed = self.parsed();
        let steps = match parsed.plan {
            Ok(steps) => steps,
            Err(problem) => {
                self.status = Some((problem, true));
                return true;
            }
        };
        self.prompt.clear();
        self.scroll = 0;
        // Running another line lets go of what `show` kept.
        self.pinned = None;
        for step in steps {
            let outcome = match step {
                Step::Quit => return false,
                Step::Show(topic) => {
                    self.pinned = Some(topic);
                    Ok(())
                }
                Step::Combat(command) => self.send(command).await,
                Step::Join(id) => match self.fetch(id).await {
                    Ok(character) => {
                        let combatant = Combatant::from_character(&character);
                        self.send(CombatCommand::AddCombatant { combatant }).await
                    }
                    Err(e) => Err(e),
                },
            };
            if let Err(e) = outcome {
                self.status = Some((format!("{}: {e:#}", parsed.sentence), true));
                return true;
            }
        }
        self.status = Some((parsed.sentence, false));
        true
    }

    /// A monster's stat block, fetched once.
    async fn monster(&mut self, id: Uuid) -> Result<Monster> {
        if let Some(monster) = self.monsters.get(&id) {
            return Ok(monster.clone());
        }
        let monster = self.login.monsters().await?.get(id).await?;
        self.monsters.insert(id, monster.clone());
        Ok(monster)
    }

    /// Sends a combat command, showing its predicted outcome at once.
    async fn send(&mut self, command: CombatCommand) -> Result<()> {
        let envelope = CommandEnvelope::new(command);
        self.replica.submit(envelope.clone())?;
        // While offline it stays pending, and is sent once reconnected.
        if let Some(sender) = &mut self.sender {
            sender.send(&envelope).await?;
        }
        Ok(())
    }

    async fn fetch(&mut self, id: Uuid) -> Result<Character> {
        let character = self.login.characters().await?.get(id).await?;
        self.sheets.insert(id, character.clone());
        Ok(character)
    }

    /// Fetches the sheets of the combatants who are characters.
    async fn refresh_sheets(&mut self) {
        let ids: Vec<Uuid> = self
            .replica
            .state()
            .combatants
            .iter()
            .filter_map(|c| c.character_id)
            .collect();
        for id in ids {
            if let Err(e) = self.fetch(id).await {
                self.status = Some((format!("couldn't load a character sheet: {e:#}"), true));
            }
        }
        let monsters: Vec<Uuid> = self
            .replica
            .state()
            .combatants
            .iter()
            .filter_map(|c| c.monster_id)
            .collect();
        for id in monsters {
            if let Err(e) = self.monster(id).await {
                self.status = Some((format!("couldn't load a stat block: {e:#}"), true));
            }
        }
    }

    async fn receive(&mut self, incoming: Incoming) {
        let message = match incoming {
            Incoming::Message(message) => message,
            Incoming::Closed(reason) => {
                self.status = Some((format!("{reason}; reconnecting…"), true));
                self.reconnect().await;
                return;
            }
            Incoming::Retry => {
                self.reconnect().await;
                return;
            }
            Incoming::Sheet(character) => {
                self.fetch_or_say(character).await;
                return;
            }
        };
        let event = match &message {
            ServerMessage::Event(event) => Some(event.clone()),
            _ => None,
        };
        match self.replica.receive(message) {
            Received::Updated => {}
            Received::Rejected { error, .. } => self.status = Some((error, true)),
            Received::NeedsResync => {
                self.reconnect().await;
                return;
            }
        }
        let Some(event) = event else { return };
        if self.log.last().is_some_and(|last| last.seq >= event.seq) {
            return;
        }
        log::learn(&mut self.names, &event.event);
        self.log.push(event.clone());
        for character in self.sheets_changed_by(&event.event) {
            self.fetch_or_say(character).await;
        }
        if let CombatEvent::Undone { command } = event.event {
            // The combat service gives back what the undone command spent
            // right after logging the undo; read those sheets again then.
            for character in self.sheets_spent_by(command) {
                let incoming = self.incoming.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let _ = incoming.send(Incoming::Sheet(character));
                });
            }
        }
        if let CombatEvent::CombatantAdded { combatant }
        | CombatEvent::CombatantRestored { combatant } = &event.event
            && let Some(monster) = combatant.monster_id
            && let Err(e) = self.monster(monster).await
        {
            self.status = Some((format!("couldn't load a stat block: {e:#}"), true));
        }
    }

    /// The characters whose sheets `event` means have changed: whoever's
    /// turn starts, who joins, and who spent a slot or uses (the combat
    /// service spends them; sheets change outside the combat too).
    fn sheets_changed_by(&self, event: &CombatEvent) -> Vec<Uuid> {
        let state = self.replica.state();
        let character = |combatant: Uuid| state.combatant(combatant).and_then(|c| c.character_id);
        let found = match event {
            CombatEvent::TurnStarted { combatant } => character(*combatant),
            CombatEvent::CombatantAdded { combatant }
            | CombatEvent::CombatantRestored { combatant } => combatant.character_id,
            other => other
                .sheet_cost()
                .and_then(|(combatant, _)| character(combatant)),
        };
        found.into_iter().collect()
    }

    /// The characters whose sheets `command` spent something from.
    fn sheets_spent_by(&self, command: Uuid) -> Vec<Uuid> {
        let state = self.replica.state();
        self.log
            .iter()
            .filter(|e| e.command_id == Some(command))
            .filter_map(|e| e.event.sheet_cost())
            .filter_map(|(combatant, _)| state.combatant(combatant).and_then(|c| c.character_id))
            .collect()
    }

    /// Reads a sheet again, saying so if that fails.
    async fn fetch_or_say(&mut self, character: Uuid) {
        if let Err(e) = self.fetch(character).await {
            self.status = Some((format!("couldn't load a character sheet: {e:#}"), true));
        }
    }

    /// Reconnects, catching the log up and resending whatever was
    /// pending. On failure, tries again a little later (the screen keeps
    /// working meanwhile, and commands queue up).
    async fn reconnect(&mut self) {
        self.sender = None;
        match self.try_reconnect().await {
            Ok(()) => {
                self.retries = 0;
                self.status = Some(("reconnected".into(), false));
            }
            Err(e) => {
                self.status = Some((format!("offline, retrying: {e:#}"), true));
                let wait = Duration::from_secs(1 << self.retries.min(4));
                self.retries += 1;
                let incoming = self.incoming.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    let _ = incoming.send(Incoming::Retry);
                });
            }
        }
    }

    async fn try_reconnect(&mut self) -> Result<()> {
        let (mut sender, snapshot) = connect(&mut self.login, self.combat, &self.incoming).await?;
        let after = self.log.last().map_or(0, |e| e.seq);
        let missed = self.login.combats().await?.log(self.combat, after).await?;
        for event in &missed {
            log::learn(&mut self.names, &event.event);
        }
        self.log.extend(missed);
        self.replica.receive(ServerMessage::Snapshot(snapshot));
        for envelope in self.replica.pending().to_vec() {
            sender.send(&envelope).await?;
        }
        self.sender = Some(sender);
        Ok(())
    }
}
