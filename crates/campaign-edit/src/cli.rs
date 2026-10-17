//! Command-line grammar.
//!
//! Two layers, like git: top-level commands (`list`, `new`) and, for anything
//! else, `<CHARACTER> <COMMAND>`. Clap hands unrecognised first words to us
//! as an external subcommand, which we then parse with [`CharacterArgs`].

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::engine::ArgValueCompleter;

use crate::complete;

/// Where the services are unless told otherwise.
pub const CHARACTER_SERVER: &str = "http://127.0.0.1:3000";
pub const COMBAT_SERVER: &str = "http://127.0.0.1:3001";
pub const AUTH_SERVER: &str = "http://127.0.0.1:3002";
pub const MONSTER_SERVER: &str = "http://127.0.0.1:3003";

const CHARACTER_HELP: &str = "\
Character commands:
  campaign-edit <CHARACTER> [COMMAND]

  <CHARACTER> is a name (case-insensitive, unique prefixes work) or an ID prefix.
  Without a command, the sheet is shown. Run `campaign-edit <CHARACTER> help`
  for the full list, e.g.

  campaign-edit grog set max-hp 50
  campaign-edit grog spell add \"Fire Bolt\" --level 0
  campaign-edit grog cast shield
  campaign-edit grog rest long";

#[derive(Debug, Parser)]
#[command(
    name = "campaign-edit",
    version,
    about = "View and edit D&D character sheets on a campaign server",
    subcommand_value_name = "COMMAND | CHARACTER",
    after_help = CHARACTER_HELP
)]
pub struct Cli {
    /// Campaign server URL.
    #[arg(
        long,
        env = "CAMPAIGN_SERVER",
        default_value = CHARACTER_SERVER,
        global = true
    )]
    pub server: String,

    /// Auth service URL, for setting up or joining a campaign. Afterwards
    /// the one this device joined through is used.
    #[arg(
        long,
        env = "CAMPAIGN_AUTH_SERVER",
        default_value = AUTH_SERVER,
        global = true
    )]
    pub auth_server: String,

    /// Monster service URL.
    #[arg(
        long,
        env = "CAMPAIGN_MONSTER_SERVER",
        default_value = MONSTER_SERVER,
        global = true
    )]
    pub monster_server: String,

    #[command(subcommand)]
    pub command: TopCommand,
}

#[derive(Debug, Subcommand)]
pub enum TopCommand {
    /// List all characters.
    #[command(visible_alias = "ls")]
    List,
    /// Create a new character.
    New(NewArgs),
    /// Create characters from JSON files, each one sheet or a list (the
    /// format `<CHARACTER> show --json` prints). Names already taken are
    /// skipped.
    Import {
        #[arg(required = true)]
        files: Vec<std::path::PathBuf>,
    },
    /// This device's identity: setting up or joining a campaign, and (for
    /// the DM) admitting and revoking devices.
    #[command(subcommand)]
    Auth(AuthCommand),
    /// The monsters the campaign can use: the shared library (the SRD) and
    /// its own.
    #[command(subcommand)]
    Monster(MonsterCommand),
    /// The spells' full descriptions: the shared library (the SRD) and the
    /// campaign's own.
    #[command(subcommand)]
    Spell(SpellTextCommand),
    /// The conditions' full descriptions (Prone and the rest), and any
    /// other named effects the campaign describes.
    #[command(subcommand)]
    Condition(ConditionCommand),
    /// Run a combat on a full-screen view: initiative order, the acting
    /// character's sheet, the log, and a command line with completion.
    Fight(FightArgs),
    /// Plan encounters ahead of time: monsters from the campaign's, how
    /// hard they'd be for the party, and a fight started from one.
    Plan(PlanArgs),
    /// A character name or ID, followed by a character command.
    #[command(external_subcommand)]
    Character(Vec<String>),
}

/// What `spell` and `condition` do, either way.
#[derive(Debug)]
pub enum TextCommand {
    List(Option<String>),
    Show(String),
    Add(std::path::PathBuf),
    Export(String),
    Delete { name: String, yes: bool },
}

#[derive(Debug, Subcommand)]
pub enum SpellTextCommand {
    /// List them, optionally only those whose name contains SEARCH.
    #[command(visible_alias = "ls")]
    List { search: Option<String> },
    /// Show a spell's description, by name (a unique prefix will do).
    Show {
        #[arg(add = ArgValueCompleter::new(complete::spells))]
        spell: String,
    },
    /// Add the campaign's own from a JSON file: one spell or a list.
    /// `export` gives a starting point.
    Add { file: std::path::PathBuf },
    /// Print a spell as JSON, e.g. to base a new one on.
    Export {
        #[arg(add = ArgValueCompleter::new(complete::spells))]
        spell: String,
    },
    /// Delete one of the campaign's own.
    Delete {
        #[arg(add = ArgValueCompleter::new(complete::spells))]
        spell: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConditionCommand {
    /// List them, optionally only those whose name contains SEARCH.
    #[command(visible_alias = "ls")]
    List { search: Option<String> },
    /// Show a condition's description, by name (a unique prefix will do).
    Show {
        #[arg(add = ArgValueCompleter::new(complete::conditions))]
        condition: String,
    },
    /// Add the campaign's own from a JSON file: one or a list, each a
    /// `name` and a `description`.
    Add { file: std::path::PathBuf },
    /// Print a condition as JSON, e.g. to base a new one on.
    Export {
        #[arg(add = ArgValueCompleter::new(complete::conditions))]
        condition: String,
    },
    /// Delete one of the campaign's own.
    Delete {
        #[arg(add = ArgValueCompleter::new(complete::conditions))]
        condition: String,
        #[arg(long)]
        yes: bool,
    },
}

impl From<SpellTextCommand> for TextCommand {
    fn from(command: SpellTextCommand) -> Self {
        match command {
            SpellTextCommand::List { search } => Self::List(search),
            SpellTextCommand::Show { spell } => Self::Show(spell),
            SpellTextCommand::Add { file } => Self::Add(file),
            SpellTextCommand::Export { spell } => Self::Export(spell),
            SpellTextCommand::Delete { spell, yes } => Self::Delete { name: spell, yes },
        }
    }
}

impl From<ConditionCommand> for TextCommand {
    fn from(command: ConditionCommand) -> Self {
        match command {
            ConditionCommand::List { search } => Self::List(search),
            ConditionCommand::Show { condition } => Self::Show(condition),
            ConditionCommand::Add { file } => Self::Add(file),
            ConditionCommand::Export { condition } => Self::Export(condition),
            ConditionCommand::Delete { condition, yes } => Self::Delete {
                name: condition,
                yes,
            },
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum MonsterCommand {
    /// List them, optionally only those whose name contains SEARCH.
    #[command(visible_alias = "ls")]
    List { search: Option<String> },
    /// Show a stat block, by name (a unique prefix will do).
    Show {
        #[arg(add = ArgValueCompleter::new(complete::monsters))]
        monster: String,
    },
    /// Add the campaign's own from a JSON file: one stat block or a list.
    /// `export` gives a starting point.
    Add { file: std::path::PathBuf },
    /// Print a stat block as JSON, e.g. to base a new monster on.
    Export {
        #[arg(add = ArgValueCompleter::new(complete::monsters))]
        monster: String,
    },
    /// Edit a stat block in $EDITOR. Editing a shared one saves a copy as
    /// the campaign's own.
    Edit {
        #[arg(add = ArgValueCompleter::new(complete::monsters))]
        monster: String,
    },
    /// Delete one of the campaign's own.
    Delete {
        #[arg(add = ArgValueCompleter::new(complete::monsters))]
        monster: String,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Args)]
pub struct FightArgs {
    /// The combat, by name or ID prefix. Defaults to the most recently
    /// active one that hasn't ended.
    #[arg(add = ArgValueCompleter::new(complete::combats))]
    pub combat: Option<String>,
    /// Start a new combat with this name instead.
    #[arg(long, value_name = "NAME", conflicts_with = "combat")]
    pub new: Option<String>,
    /// Start a new combat from a planned encounter (see `plan`), with its
    /// monsters in it.
    #[arg(
        long,
        value_name = "PLAN",
        conflicts_with_all = ["combat", "new"],
        add = ArgValueCompleter::new(complete::encounters)
    )]
    pub plan: Option<String>,
    /// Combat service URL.
    #[arg(
        long,
        env = "CAMPAIGN_COMBAT_SERVER",
        default_value = COMBAT_SERVER
    )]
    pub combat_server: String,
}

#[derive(Debug, Args)]
pub struct PlanArgs {
    /// The plan to open, by name (a new one if there's none by that name).
    /// Defaults to the most recently changed one.
    #[arg(add = ArgValueCompleter::new(complete::encounters))]
    pub plan: Option<String>,
    /// Combat service URL (it keeps the plans).
    #[arg(
        long,
        env = "CAMPAIGN_COMBAT_SERVER",
        default_value = COMBAT_SERVER
    )]
    pub combat_server: String,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Show whether this device is in a campaign, and as whom.
    Status,
    /// List the campaigns this device is in; * marks the one in use.
    Campaigns,
    /// Switch to another campaign this device is in, by name (a unique
    /// prefix is enough) or ID prefix.
    Use {
        #[arg(add = ArgValueCompleter::new(complete::campaigns))]
        campaign: String,
    },
    /// Forget a campaign on this device. The device stays registered in
    /// it (the DM can revoke it); joining again brings it back.
    Forget {
        #[arg(add = ArgValueCompleter::new(complete::campaigns))]
        campaign: String,
    },
    /// DM: show a short code for a new device to join with, and wait for
    /// it. The code works once and for ten minutes.
    Invite {
        /// Make it a DM device (e.g. your own second device).
        #[arg(long)]
        dm: bool,
        /// Approve whoever joins without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Join a campaign with a code from your DM's `auth invite`. Joining
    /// another campaign adds it and switches to it (see `auth use`).
    Pair {
        /// The code, e.g. 7-tavern-dragon.
        code: String,
        /// What to call this device, e.g. "Vex's laptop".
        #[arg(long)]
        name: String,
    },
    /// Show this device's public key, to give to your DM (the manual
    /// alternative to `invite`/`pair`).
    Key,
    /// DM: make this device the DM of a new campaign, with the one-time code
    /// printed by `auth-service new-campaign`.
    Setup {
        // Codes and keys are base64url, so they may start with `-`.
        #[arg(allow_hyphen_values = true)]
        code: String,
        /// What to call this device, e.g. "DM laptop".
        #[arg(long)]
        name: String,
    },
    /// Join a campaign after the DM has approved this device's key.
    Join {
        /// The campaign id the DM gave you.
        campaign: uuid::Uuid,
    },
    /// DM: admit a device, by the key its owner got from `auth key`.
    Approve {
        #[arg(allow_hyphen_values = true)]
        public_key: campaign_auth::PublicKey,
        /// Whose device it is, e.g. "Vex's laptop".
        #[arg(long)]
        name: String,
        /// Make it a DM device (e.g. your own second device).
        #[arg(long)]
        dm: bool,
    },
    /// List the campaign's devices, checking their certificates.
    Devices,
    /// DM: lock a device out, by its name or key (a unique prefix is enough).
    Revoke {
        #[arg(allow_hyphen_values = true)]
        device: String,
    },
}

#[derive(Debug, Args)]
pub struct NewArgs {
    pub name: String,
    #[arg(long)]
    pub race: Option<String>,
    #[arg(long)]
    pub player: Option<String>,
    #[arg(long)]
    pub background: Option<String>,
}

// The arguments after `campaign-edit <CHARACTER>`.
#[derive(Debug, Parser)]
#[command(
    no_binary_name = true,
    disable_version_flag = true,
    about = "View and edit one character"
)]
pub struct CharacterArgs {
    #[command(subcommand)]
    pub command: Option<CharacterCommand>,
}

#[derive(Debug, Subcommand)]
pub enum CharacterCommand {
    /// Show the character sheet (the default).
    Show {
        /// Print the raw JSON instead.
        #[arg(long)]
        json: bool,
    },
    /// Print a single field.
    Get { field: Field },
    /// Set a single field.
    Set {
        field: Field,
        /// The new value. For hp, +N heals and -N deals damage.
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
    /// Edit the whole sheet as JSON in $VISUAL / $EDITOR.
    Edit,
    /// Delete the character.
    #[command(visible_alias = "rm")]
    Delete {
        /// Don't ask for confirmation.
        #[arg(short, long)]
        yes: bool,
    },

    /// Who plays the character: the player devices that may change it and
    /// act for it in a fight. Only a DM device changes this.
    Owners {
        #[command(subcommand)]
        change: Option<OwnersCommand>,
    },

    /// Manage class levels.
    #[command(subcommand)]
    Class(ClassCommand),
    /// Manage skill proficiencies.
    #[command(subcommand)]
    Skill(ProficiencyCommand),
    /// Manage saving throw proficiencies.
    #[command(subcommand)]
    Save(ProficiencyCommand),
    /// Manage inventory.
    #[command(subcommand)]
    Item(ItemCommand),
    /// Manage the spell list.
    #[command(subcommand)]
    Spell(SpellCommand),
    /// Spend and regain spell slots. How many there are follows from the
    /// classes (and the campaign's edition).
    #[command(subcommand)]
    Slots(SlotsCommand),
    /// Spend and regain Pact Magic slots, which follow from Warlock levels.
    #[command(subcommand)]
    Pact(PactCommand),
    /// Manage limited-use features (Action Surge, Rage, Ki, ...).
    #[command(subcommand)]
    Resource(ResourceCommand),

    /// Cast a spell, spending the right slot.
    Cast {
        spell: String,
        /// Slot level to cast at (upcasting).
        #[arg(long)]
        at: Option<u8>,
        /// Use a Pact Magic slot.
        #[arg(long, conflicts_with = "ritual")]
        pact: bool,
        /// Cast as a ritual (no slot).
        #[arg(long)]
        ritual: bool,
    },
    /// Spend uses of a resource.
    Use {
        resource: String,
        #[arg(default_value_t = 1)]
        amount: u16,
    },
    /// Regain uses of a resource.
    Regain {
        resource: String,
        #[arg(default_value_t = 1)]
        amount: u16,
    },
    /// Take a short or long rest.
    Rest { kind: RestArg },
}

/// Sheet fields addressable with `get` / `set`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Field {
    Name,
    Player,
    Race,
    Background,
    Alignment,
    #[value(alias = "experience")]
    Xp,
    #[value(alias = "armor-class")]
    Ac,
    Speed,
    MaxHp,
    Hp,
    TempHp,
    #[value(alias = "strength")]
    Str,
    #[value(alias = "dexterity")]
    Dex,
    #[value(alias = "constitution")]
    Con,
    #[value(alias = "intelligence")]
    Int,
    #[value(alias = "wisdom")]
    Wis,
    #[value(alias = "charisma")]
    Cha,
    Notes,
}

#[derive(Debug, Subcommand)]
pub enum ClassCommand {
    /// Add a class (or levels in it).
    Add {
        name: String,
        #[arg(default_value_t = 1)]
        level: u8,
        #[arg(long)]
        subclass: Option<String>,
        /// Spellcasting ability, e.g. int, wis, cha.
        #[arg(long, value_parser = crate::parse::ability)]
        casting: Option<campaign_types::Ability>,
    },
    /// Set the level of an existing class.
    Level { name: String, level: u8 },
    /// Set or clear the subclass.
    Subclass {
        name: String,
        subclass: Option<String>,
    },
    /// Remove a class.
    #[command(visible_alias = "rm")]
    Remove { name: String },
}

#[derive(Debug, Subcommand)]
pub enum ProficiencyCommand {
    /// Add proficiency.
    Add {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove proficiency.
    #[command(visible_alias = "rm")]
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ItemCommand {
    /// Add an item (stacks with an existing one of the same name).
    Add {
        name: String,
        #[arg(short = 'n', long, default_value_t = 1)]
        quantity: u32,
        #[arg(long)]
        notes: Option<String>,
    },
    /// Remove some or all of an item.
    #[command(visible_alias = "rm")]
    Remove {
        name: String,
        /// How many to remove; all of them if omitted.
        #[arg(short = 'n', long)]
        quantity: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SpellCommand {
    /// Add a spell to the spell list.
    Add {
        name: String,
        /// Spell level (0 for cantrips).
        #[arg(short, long, default_value_t = 0)]
        level: u8,
        /// Class that grants the spell.
        #[arg(long)]
        class: Option<String>,
        #[arg(long)]
        concentration: bool,
        #[arg(long)]
        ritual: bool,
        /// Add without preparing it.
        #[arg(long)]
        unprepared: bool,
    },
    /// Remove a spell.
    #[command(visible_alias = "rm")]
    Remove { name: String },
    /// Mark a spell as prepared.
    Prepare { name: String },
    /// Mark a spell as not prepared.
    Unprepare { name: String },
}

#[derive(Debug, Subcommand)]
pub enum SlotsCommand {
    /// Spend a slot without casting a listed spell.
    Use { level: u8 },
    /// Regain a spent slot.
    Regain { level: u8 },
}

#[derive(Debug, Subcommand)]
pub enum PactCommand {
    /// Spend a pact slot.
    Use,
    /// Regain a pact slot.
    Regain,
}

#[derive(Debug, Subcommand)]
pub enum OwnersCommand {
    /// Let a device play the character, by name or key prefix (see `auth
    /// devices`).
    Add { device: String },
    /// Stop a device playing the character.
    #[command(visible_alias = "rm")]
    Remove { device: String },
}

#[derive(Debug, Subcommand)]
pub enum ResourceCommand {
    /// Add a limited-use feature.
    Add {
        name: String,
        max: u16,
        #[arg(long, value_enum, default_value_t = RechargeArg::Long)]
        recharge: RechargeArg,
    },
    /// Change how many uses a feature has.
    Max { name: String, max: u16 },
    /// Change when a feature recharges.
    Recharge { name: String, recharge: RechargeArg },
    /// Remove a feature.
    #[command(visible_alias = "rm")]
    Remove { name: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RechargeArg {
    Short,
    Long,
    Manual,
}

impl From<RechargeArg> for campaign_types::Recharge {
    fn from(r: RechargeArg) -> Self {
        match r {
            RechargeArg::Short => Self::ShortRest,
            RechargeArg::Long => Self::LongRest,
            RechargeArg::Manual => Self::Manual,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RestArg {
    Short,
    Long,
}

impl From<RestArg> for campaign_types::RestKind {
    fn from(r: RestArg) -> Self {
        match r {
            RestArg::Short => Self::Short,
            RestArg::Long => Self::Long,
        }
    }
}
