mod auth;
mod cli;
mod display;
mod edit;
mod editor;
mod fight;
mod identity;
mod monsters;
mod parse;

use std::process::ExitCode;

use anyhow::{Result, bail};
use campaign_client::Client;
use campaign_types::{Character, CharacterAction, CharacterSheet};
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum};
use uuid::Uuid;

use cli::{CharacterArgs, CharacterCommand, Cli, PactCommand, SlotsCommand, TopCommand};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start async runtime");
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    if let TopCommand::Auth(command) = cli.command {
        return auth::run(command, &cli.auth_server).await;
    }
    if let TopCommand::Fight(args) = cli.command {
        return fight::run(args, &cli.server, &cli.monster_server).await;
    }
    if let TopCommand::Monster(command) = cli.command {
        let (_, session) = identity::Identity::locate()?.login().await?;
        let client = campaign_client::monsters::MonsterClient::new(&cli.monster_server)
            .with_token(&session.token);
        return monsters::run(&client, command).await;
    }
    let (_, session) = identity::Identity::locate()?.login().await?;
    let client = Client::new(&cli.server).with_token(&session.token);
    match cli.command {
        TopCommand::Auth(_) | TopCommand::Fight(_) | TopCommand::Monster(_) => {
            unreachable!("handled above")
        }
        TopCommand::List => println!("{}", display::list(&client.list().await?)),
        TopCommand::New(args) => {
            let sheet = CharacterSheet {
                name: args.name,
                race: args.race.unwrap_or_default(),
                player_name: args.player,
                background: args.background.unwrap_or_default(),
                ..Default::default()
            };
            let created = client.create(&sheet).await?;
            println!(
                "Created {} ({})",
                created.sheet.name,
                parse::short_id(created.id)
            );
        }
        TopCommand::Character(args) => character(&client, args).await?,
    }
    Ok(())
}

async fn character(client: &Client, args: Vec<String>) -> Result<()> {
    let (query, rest) = args
        .split_first()
        .expect("clap always passes the subcommand name");

    // Parse before touching the network so `help` and usage errors are instant.
    let mut command = CharacterArgs::command().bin_name(format!("campaign-edit {query}"));
    let matches = command
        .try_get_matches_from_mut(rest)
        .unwrap_or_else(|e| e.exit());
    let args = CharacterArgs::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    let id = parse::resolve_character(query, &client.list().await?)?;
    let command = args
        .command
        .unwrap_or(CharacterCommand::Show { json: false });

    use CharacterCommand as C;
    match command {
        C::Show { json } => {
            let character = client.get(id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&character)?);
            } else {
                println!("{}", display::sheet(&character));
            }
        }
        C::Get { field } => println!("{}", edit::get_field(&client.get(id).await?.sheet, field)),
        C::Set { field, value } => {
            let saved = modify(client, id, |s| edit::set_field(s, field, &value)).await?;
            let name = field.to_possible_value().expect("no skipped fields");
            let shown = edit::get_field(&saved.sheet, field);
            println!("{}: {} = {shown}", saved.sheet.name, name.get_name());
        }
        C::Edit => match editor::edit(client, client.get(id).await?).await? {
            Some(saved) => println!("Saved {} (v{})", saved.sheet.name, saved.version),
            None => println!("No changes."),
        },
        C::Delete { yes } => {
            let name = client.get(id).await?.sheet.name;
            if !yes && !editor::confirm(&format!("Delete {name}? This cannot be undone."))? {
                bail!("not deleted (pass --yes to skip the prompt)");
            }
            client.delete(id).await?;
            println!("Deleted {name}");
        }

        C::Class(c) => {
            // The classes decide the spell slots; show what they come to.
            let saved = modify(client, id, |s| edit::class(s, &c)).await?;
            updated(saved.clone());
            if display::slots_line(&saved.sheet).is_some() {
                print_slots(&saved);
            }
        }
        C::Skill(c) => updated(modify(client, id, |s| edit::skills(s, &c)).await?),
        C::Save(c) => updated(modify(client, id, |s| edit::saves(s, &c)).await?),
        C::Item(c) => updated(modify(client, id, |s| edit::item(s, &c)).await?),
        C::Spell(c) => updated(modify(client, id, |s| edit::spell(s, &c)).await?),
        C::Resource(c) => updated(modify(client, id, |s| edit::resource(s, &c)).await?),

        C::Slots(SlotsCommand::Use { level }) => {
            print_slots(&act(client, id, CharacterAction::ExpendSpellSlot { level }).await?)
        }
        C::Slots(SlotsCommand::Regain { level }) => {
            print_slots(&act(client, id, CharacterAction::RestoreSpellSlot { level }).await?)
        }
        C::Pact(PactCommand::Use) => {
            print_slots(&act(client, id, CharacterAction::ExpendPactSlot).await?)
        }
        C::Pact(PactCommand::Regain) => {
            print_slots(&act(client, id, CharacterAction::RestorePactSlot).await?)
        }
        C::Cast {
            spell,
            at,
            pact,
            ritual,
        } => {
            let action = CharacterAction::CastSpell {
                spell: spell.clone(),
                at_level: at,
                pact,
                ritual,
            };
            let saved = act(client, id, action).await?;
            let found = saved.sheet.spellcasting.find_spell(&spell);
            let name = found.map_or(spell.as_str(), |s| s.name.as_str());
            if ritual || found.is_some_and(|s| s.level == 0) {
                println!("Cast {name} (no slot used)");
            } else {
                println!("Cast {name}");
                print_slots(&saved);
            }
        }
        C::Use { resource, amount } => {
            let action = CharacterAction::UseResource {
                name: resource.clone(),
                amount,
            };
            print_resource(&act(client, id, action).await?, &resource);
        }
        C::Regain { resource, amount } => {
            let action = CharacterAction::RestoreResource {
                name: resource.clone(),
                amount,
            };
            print_resource(&act(client, id, action).await?, &resource);
        }
        C::Rest { kind } => {
            let saved = act(client, id, CharacterAction::Rest { kind: kind.into() }).await?;
            let label = format!("{kind:?}").to_lowercase();
            println!("{} takes a {label} rest.\n", saved.sheet.name);
            println!("{}", display::status(&saved.sheet));
        }
    }
    Ok(())
}

/// Structural edit via fetch → change → PUT with If-Match, re-applying the
/// change automatically if someone else saved in the meantime.
async fn modify(
    client: &Client,
    id: Uuid,
    change: impl FnMut(&mut CharacterSheet) -> Result<()>,
) -> Result<Character> {
    client.modify(id, change).await
}

async fn act(client: &Client, id: Uuid, action: CharacterAction) -> Result<Character> {
    Ok(client.act(id, &action).await?)
}

fn updated(saved: Character) {
    println!("Updated {} (v{})", saved.sheet.name, saved.version);
}

fn print_resource(saved: &Character, name: &str) {
    let sheet = &saved.sheet;
    if let Some(r) = sheet
        .resources
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(name))
    {
        println!("{}  {}", r.name, display::pool(r.uses));
    }
}

fn print_slots(saved: &Character) {
    match display::slots_line(&saved.sheet) {
        Some(slots) => println!("Slots  {slots}"),
        None => println!("{} has no spell slots", saved.sheet.name),
    }
}
