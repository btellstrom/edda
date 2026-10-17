//! A terminal screen for running a combat: the initiative order, the sheet
//! of whoever is acting, the log, and a command line that completes
//! actions, spells and combatants and says who does what to whom before
//! anything is sent.

pub mod command;
mod log;
mod ui;

use std::{collections::HashMap, time::Duration};

use anyhow::{Context as _, Result, anyhow, bail};
use campaign_auth::protocol::Session;
use campaign_client::{
    Client,
    combat::{CombatClient, Received, Replica, live::CommandSender},
    monsters::MonsterClient,
};
use campaign_types::{
    Character, CharacterAction, CharacterSummary,
    combat::{
        CombatCommand, CombatEvent, CombatStatus, CombatSummary, Combatant, CommandEnvelope,
        LoggedEvent, ServerMessage, SpellSlot,
    },
    encounter::EncounterPlan,
    monster::{Monster, MonsterSummary},
    rules_text::{ConditionEntry, ConditionText, SpellEntry, SpellText},
};
use chrono::Utc;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    cli::FightArgs,
    identity::{Identity, Profile},
};
use command::{Context, Parsed, Step, Suggestion, Topic};

/// Logged in, with the token renewed shortly before it expires (tokens last
/// a quarter of an hour; a fight lasts longer). The planner uses it too.
pub(crate) struct Login {
    identity: Identity,
    /// The campaign the screen was opened in, even if another is chosen
    /// meanwhile (say, in another terminal).
    pub(crate) profile: Profile,
    pub(crate) session: Session,
    characters_url: String,
    combat_url: String,
    monsters_url: String,
}

impl Login {
    pub(crate) async fn new(
        characters_url: &str,
        combat_url: &str,
        monsters_url: &str,
    ) -> Result<Self> {
        let identity = Identity::locate()?;
        let (profile, session) = identity.login().await?;
        Ok(Self {
            identity,
            profile,
            session,
            characters_url: characters_url.to_string(),
            combat_url: combat_url.to_string(),
            monsters_url: monsters_url.to_string(),
        })
    }

    async fn token(&mut self) -> Result<String> {
        if self.session.claims.expires_at < Utc::now() + chrono::Duration::minutes(2) {
            self.session = self.identity.login_to(&self.profile).await?;
        }
        Ok(self.session.token.clone())
    }

    pub(crate) async fn characters(&mut self) -> Result<Client> {
        Ok(Client::new(&self.characters_url).with_token(&self.token().await?))
    }

    pub(crate) async fn combats(&mut self) -> Result<CombatClient> {
        Ok(CombatClient::new(&self.combat_url).with_token(&self.token().await?))
    }

    pub(crate) async fn monsters(&mut self) -> Result<MonsterClient> {
        Ok(MonsterClient::new(&self.monsters_url).with_token(&self.token().await?))
    }
}

/// Something a cast or feature use spent on a character sheet.
enum Spent {
    Slot(SpellSlot),
    Feature(String, u16),
}

/// What the WebSocket reader passes to the screen.
enum Incoming {
    Message(ServerMessage),
    Closed(String),
    /// Time to try reconnecting again.
    Retry,
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
    input: String,
    selected: usize,
    /// The outcome of the last line, and whether it was a failure.
    status: Option<(String, bool)>,
    /// How many lines the log is scrolled back.
    scroll: usize,
    /// The sheet panel, scrolled by hand.
    panel: crate::styled::PanelScroll,
    /// Failed reconnection attempts in a row.
    retries: u32,
}

pub async fn run(args: FightArgs, characters_url: &str, monsters_url: &str) -> Result<()> {
    let mut login = Login::new(characters_url, &args.combat_server, monsters_url).await?;
    let combats = login.combats().await?;
    let plan = match &args.plan {
        Some(wanted) => {
            let encounters = combats.encounters().await?;
            let found =
                crate::plan::command::find_plan(&encounters, wanted, false).ok_or_else(|| {
                    anyhow!("no plan matches \"{wanted}\" (see `campaign-edit plan`)")
                })?;
            Some(found.plan.clone())
        }
        None => None,
    };
    let combat = match (&args.new, &plan) {
        (Some(name), _) => combats.create(name).await?.id,
        (None, Some(plan)) => combats.create(&plan.name).await?.id,
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
        input: String::new(),
        selected: 0,
        status: monster_problem.map(|problem| (problem, true)),
        scroll: 0,
        panel: Default::default(),
        retries: 0,
    };
    app.refresh_sheets().await;
    if let Some(plan) = plan {
        app.load(&plan).await;
    }

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
                .nth(self.selected)
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
        command::parse(&self.input, &self.context())
    }

    pub fn suggestions(&self) -> Vec<Suggestion> {
        command::suggest(&self.input, &self.context())
    }

    /// The combatant whose sheet to show: whoever the line is about.
    pub fn shown(&self) -> Option<&Combatant> {
        let id = self.parsed().actor?;
        self.replica.state().combatant(id)
    }

    /// Handles a key; `false` to leave.
    async fn key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Up if alt => {
                self.panel.by(-1);
                return true;
            }
            KeyCode::Down if alt => {
                self.panel.by(1);
                return true;
            }
            KeyCode::Char('c' | 'd') if ctrl => return false,
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
            // A second Esc, on an empty line, lets go of what `show` kept.
            KeyCode::Esc if self.input.is_empty() => self.pinned = None,
            KeyCode::Esc => self.input.clear(),
            KeyCode::Tab => {
                if let Some(s) = self.suggestions().get(self.selected) {
                    self.input = s.line.clone();
                }
            }
            KeyCode::Down => self.selected += 1,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::PageUp => self.scroll += 5,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(5),
            KeyCode::Enter => return self.enter().await,
            _ => return true,
        }
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.selected = 0;
        }
        let count = self.suggestions().len();
        self.selected = self.selected.min(count.saturating_sub(1));
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
        self.input.clear();
        self.selected = 0;
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
                Step::Summon {
                    monster,
                    count,
                    roll,
                } => self.summon(monster, count, roll).await,
                Step::Character { id, action } => {
                    let acted = match self.login.characters().await {
                        Ok(client) => client.act(id, &action).await.map_err(Into::into),
                        Err(e) => Err(e),
                    };
                    acted.map(|character| {
                        self.sheets.insert(id, character);
                    })
                }
            };
            if let Err(e) = outcome {
                self.status = Some((format!("{}: {e:#}", parsed.sentence), true));
                return true;
            }
        }
        self.status = Some((parsed.sentence, false));
        true
    }

    /// Puts a plan's monsters in the fight.
    async fn load(&mut self, plan: &EncounterPlan) {
        let mut missing = Vec::new();
        for entry in &plan.monsters {
            if self
                .summon(entry.monster, entry.count, entry.roll)
                .await
                .is_err()
            {
                missing.push(entry.name.clone());
            }
        }
        self.status = Some(if missing.is_empty() {
            (format!("{} is set up from its plan", plan.name), false)
        } else {
            (
                format!(
                    "couldn't add {} (deleted from the monsters?)",
                    missing.join(", ")
                ),
                true,
            )
        });
    }

    /// Adds `count` of a monster, numbered on from any already in the fight,
    /// at their maximum hit points or with them rolled.
    async fn summon(&mut self, id: Uuid, count: u16, roll: bool) -> Result<()> {
        let monster = self.monster(id).await?;
        let names = command::monster_names(&monster.sheet.name, count, self.replica.state());
        for name in names {
            let hit_points = match (roll, monster.sheet.hit_dice()) {
                (true, Some(dice)) => dice.roll(|sides| rand::random_range(1..=sides)),
                _ => monster.sheet.max_hit_points(),
            };
            let combatant = Combatant::from_monster(&monster, name, hit_points);
            self.send(CombatCommand::AddCombatant { combatant }).await?;
        }
        Ok(())
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
            let _ = self.monster(id).await;
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
        };
        let event = match &message {
            ServerMessage::Event(event) => Some(event.clone()),
            _ => None,
        };
        // An undo this screen sent: what the undone command spent on a
        // sheet is given back here, as the combat can't do it.
        let undone = match &event {
            Some(LoggedEvent {
                command_id: Some(id),
                event: CombatEvent::Undone { command },
                ..
            }) if self.replica.pending().iter().any(|p| p.command_id == *id) => Some(*command),
            _ => None,
        };
        match self.replica.receive(message) {
            Received::Updated => {}
            Received::Rejected { envelope, error } => {
                self.status = Some((error, true));
                // Refused, so it never happened: give back what it spent.
                let spent = match envelope.command {
                    CombatCommand::CastSpell {
                        caster,
                        slot: Some(slot),
                        ..
                    } => Some((caster, Spent::Slot(slot))),
                    CombatCommand::UseFeature {
                        combatant,
                        feature,
                        amount,
                    } => Some((combatant, Spent::Feature(feature, amount))),
                    _ => None,
                };
                if let Some((combatant, spent)) = spent {
                    self.give_back(combatant, spent).await;
                }
            }
            Received::NeedsResync => {
                self.reconnect().await;
                return;
            }
        }
        if let Some(command) = undone {
            let spent: Vec<_> = self
                .log
                .iter()
                .filter(|e| e.command_id == Some(command))
                .filter_map(|e| match &e.event {
                    CombatEvent::SpellCast {
                        caster,
                        slot: Some(slot),
                        ..
                    } => Some((*caster, Spent::Slot(*slot))),
                    CombatEvent::FeatureUsed {
                        combatant,
                        feature,
                        amount,
                    } => Some((*combatant, Spent::Feature(feature.clone(), *amount))),
                    _ => None,
                })
                .collect();
            for (combatant, spent) in spent {
                self.give_back(combatant, spent).await;
            }
        }
        let Some(event) = event else { return };
        if self.log.last().is_some_and(|last| last.seq >= event.seq) {
            return;
        }
        log::learn(&mut self.names, &event.event);
        self.log.push(event.clone());
        // Sheets change outside the combat too (slots, features, HP
        // written back); catch up with whoever's turn it now is, and with
        // characters who join.
        use CombatEvent as E;
        let character = match &event.event {
            E::TurnStarted { combatant } => self
                .replica
                .state()
                .combatant(*combatant)
                .and_then(|c| c.character_id),
            E::CombatantAdded { combatant } | E::CombatantRestored { combatant } => {
                if let Some(monster) = combatant.monster_id {
                    let _ = self.monster(monster).await;
                }
                combatant.character_id
            }
            _ => None,
        };
        if let Some(id) = character {
            let _ = self.fetch(id).await;
        }
    }

    /// Restores a spent slot or feature use on the combatant's sheet.
    async fn give_back(&mut self, combatant: Uuid, spent: Spent) {
        let state = self.replica.state();
        let Some(character) = state.combatant(combatant).and_then(|c| c.character_id) else {
            return;
        };
        let (action, what) = match spent {
            Spent::Slot(SpellSlot::Level(level)) => (
                CharacterAction::RestoreSpellSlot { level },
                format!("a {} level slot", crate::display::ordinal(level)),
            ),
            Spent::Slot(SpellSlot::Pact(_)) => {
                (CharacterAction::RestorePactSlot, "a pact slot".to_string())
            }
            Spent::Feature(name, amount) => (
                CharacterAction::RestoreResource {
                    name: name.clone(),
                    amount,
                },
                name,
            ),
        };
        let restored = match self.login.characters().await {
            Ok(client) => client.act(character, &action).await.map_err(Into::into),
            Err(e) => Err(e),
        };
        match restored {
            Ok(saved) => {
                let name = saved.sheet.name.clone();
                self.sheets.insert(character, saved);
                let note = format!("gave {name} back {what}");
                self.status = Some(match self.status.take() {
                    Some((text, failed)) => (format!("{text}; {note}"), failed),
                    None => (note, false),
                });
            }
            Err(e) => {
                self.status = Some((format!("couldn't give back {what}: {e:#}"), true));
            }
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
