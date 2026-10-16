//! Command-line grammar.
//!
//! Two layers, like git: top-level commands (`list`, `new`) and, for anything
//! else, `<CHARACTER> <COMMAND>`. Clap hands unrecognised first words to us
//! as an external subcommand, which we then parse with [`CharacterArgs`].

use clap::{Args, Parser, Subcommand, ValueEnum};

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
        default_value = "http://127.0.0.1:3000",
        global = true
    )]
    pub server: String,

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
    /// A character name or ID, followed by a character command.
    #[command(external_subcommand)]
    Character(Vec<String>),
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
    /// Manage and spend spell slots.
    #[command(subcommand)]
    Slots(SlotsCommand),
    /// Manage and spend Pact Magic slots.
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
    /// Set the number of slots at a level (0 removes the level).
    Set { level: u8, max: u16 },
    /// Spend a slot without casting a listed spell.
    Use { level: u8 },
    /// Regain a spent slot.
    Regain { level: u8 },
}

#[derive(Debug, Subcommand)]
pub enum PactCommand {
    /// Set pact slot level and count.
    Set { level: u8, max: u16 },
    /// Remove Pact Magic.
    #[command(visible_alias = "rm")]
    Remove,
    /// Spend a pact slot.
    Use,
    /// Regain a pact slot.
    Regain,
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
