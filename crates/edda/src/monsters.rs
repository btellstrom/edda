//! `edda monster ...`: browsing the monsters a campaign can use,
//! and keeping its own.

use std::path::Path;

use anyhow::{Context, Result, bail};
use edda_client::{ClientError, monsters::MonsterClient};
use edda_types::monster::{Monster, MonsterSheet, MonsterSummary};
use serde_json::Value;

use crate::{
    cli::MonsterCommand,
    display,
    editor::{self, confirm},
};

pub async fn run(client: &MonsterClient, command: MonsterCommand) -> Result<()> {
    match command {
        MonsterCommand::List { search } => {
            let monsters = client.list(search.as_deref()).await?;
            println!("{}", display::monster_list(&monsters));
        }
        MonsterCommand::Show { monster } => {
            let monster = find(client, &monster).await?;
            crate::styled::print(&display::monster(
                &monster.sheet,
                crate::styled::print_width(),
                display::Detail::Full,
            ));
        }
        MonsterCommand::Add { file } => {
            for sheet in read(&file)? {
                let added = client.create(&sheet).await?;
                println!("Added {} to the campaign", added.sheet.name);
            }
        }
        MonsterCommand::Export { monster } => {
            let monster = find(client, &monster).await?;
            println!("{}", serde_json::to_string_pretty(&monster.sheet)?);
        }
        MonsterCommand::Edit { monster } => {
            let monster = find(client, &monster).await?;
            match edit(client, monster).await? {
                Some(saved) => println!("Saved {} (v{})", saved.sheet.name, saved.version),
                None => println!("No changes."),
            }
        }
        MonsterCommand::Delete { monster, yes } => {
            let monster = find(client, &monster).await?;
            if monster.campaign.is_none() {
                bail!(
                    "{} is in the shared library, not the campaign's own",
                    monster.sheet.name
                );
            }
            let name = &monster.sheet.name;
            if !yes && !confirm(&format!("Delete {name} from the campaign?"))? {
                bail!("not deleted (pass --yes to skip the prompt)");
            }
            client.delete(monster.id).await?;
            println!("Deleted {name}");
        }
    }
    Ok(())
}

/// The monster `query` names: its name (any case; the campaign's own before
/// a shared one of the same name), a unique prefix of it, or an ID prefix.
pub async fn find(client: &MonsterClient, query: &str) -> Result<Monster> {
    let monsters = client.list(None).await?;
    let summary = pick(&monsters, query)?;
    Ok(client.get(summary.id).await?)
}

fn pick<'m>(monsters: &'m [MonsterSummary], query: &str) -> Result<&'m MonsterSummary> {
    let query_lower = query.to_lowercase();
    let mut exact: Vec<_> = monsters
        .iter()
        .filter(|m| m.name.to_lowercase() == query_lower)
        .collect();
    exact.sort_by_key(|m| m.shared);
    if let Some(first) = exact.first() {
        return Ok(first);
    }
    let matching: Vec<_> = monsters
        .iter()
        .filter(|m| {
            m.name.to_lowercase().starts_with(&query_lower) || m.id.to_string().starts_with(query)
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(one),
        [] => bail!("no monster called {query:?}; `edda monster list` shows them"),
        many if many.len() <= 8 => {
            let names: Vec<_> = many.iter().map(|m| m.name.as_str()).collect();
            bail!("{query:?} could be {}", names.join(", "))
        }
        many => bail!(
            "{query:?} matches {} monsters; type more of the name",
            many.len()
        ),
    }
}

/// Stat blocks from a file: one, or a list.
fn read(path: &Path) -> Result<Vec<MonsterSheet>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&text).with_context(|| format!("in {}", path.display()))
}

/// Parses stat blocks, rejecting unknown fields so a typo like
/// `"armour_class"` isn't silently dropped, and checks each one.
fn parse(text: &str) -> Result<Vec<MonsterSheet>> {
    let input: Value = serde_json::from_str(text).context("invalid JSON")?;
    let items = match input {
        Value::Array(items) => items,
        single => vec![single],
    };
    items
        .into_iter()
        .enumerate()
        .map(|(n, item)| {
            let sheet: MonsterSheet = serde_json::from_value(item.clone())
                .with_context(|| format!("monster {}", n + 1))?;
            let label = if sheet.name.is_empty() {
                format!("monster {}", n + 1)
            } else {
                sheet.name.clone()
            };
            let unknown = editor::unknown_keys(&item, &serde_json::to_value(&sheet)?, "");
            if !unknown.is_empty() {
                bail!("{label}: unknown field(s): {}", unknown.join(", "));
            }
            sheet.validate().with_context(|| label)?;
            Ok(sheet)
        })
        .collect()
}

/// The stat block in $EDITOR. Editing a shared monster saves a copy as
/// the campaign's own, leaving the shared one as it is.
async fn edit(client: &MonsterClient, monster: Monster) -> Result<Option<Monster>> {
    let path = std::env::temp_dir().join(format!("edda-monster-{}.json", monster.id));
    std::fs::write(&path, serde_json::to_string_pretty(&monster.sheet)? + "\n")
        .with_context(|| format!("writing {}", path.display()))?;
    loop {
        editor::open_editor(&path)?;
        let text = std::fs::read_to_string(&path)?;
        let sheet = match parse(&text).and_then(|mut sheets| match sheets.len() {
            1 => Ok(sheets.remove(0)),
            _ => bail!("edit one monster at a time"),
        }) {
            Ok(sheet) => sheet,
            Err(e) => {
                eprintln!("error: {e:#}");
                if confirm("Edit again?")? {
                    continue;
                }
                bail!("aborted; your edits are kept in {}", path.display());
            }
        };
        if sheet == monster.sheet {
            std::fs::remove_file(&path).ok();
            return Ok(None);
        }
        let saved = if monster.campaign.is_some() {
            client.replace(monster.id, &sheet).await
        } else {
            let mut copy = sheet;
            if copy.source == monster.sheet.source {
                copy.source = format!("Homebrew (from {})", monster.sheet.source);
            }
            client.create(&copy).await
        };
        return match saved {
            Ok(saved) => {
                std::fs::remove_file(&path).ok();
                Ok(Some(saved))
            }
            Err(ClientError::Rejected(message)) => {
                bail!("{message}\nYour edits are kept in {}", path.display())
            }
            Err(e) => Err(e.into()),
        };
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn summary(name: &str, shared: bool) -> MonsterSummary {
        MonsterSummary {
            id: Uuid::new_v4(),
            name: name.into(),
            size: "Small".into(),
            kind: "humanoid".into(),
            challenge: "1/4".into(),
            hit_points: 7,
            max_hit_points: 12,
            xp: 50,
            armor_class: 15,
            source: "SRD 5.2".into(),
            shared,
        }
    }

    #[test]
    fn names_pick_one_monster_preferring_the_campaigns_own() {
        let monsters = [
            summary("Goblin Boss", true),
            summary("Goblin Minion", true),
            summary("Goblin Warrior", true),
            summary("Goblin Warrior", false),
        ];
        let own = pick(&monsters, "goblin warrior").unwrap();
        assert!(!own.shared, "the campaign's own first");
        assert_eq!(pick(&monsters, "goblin b").unwrap().name, "Goblin Boss");
        let err = pick(&monsters, "goblin").unwrap_err().to_string();
        assert!(err.contains("could be"), "{err}");
        assert!(pick(&monsters, "troll").is_err());
    }

    #[test]
    fn files_hold_one_or_many_monsters_and_typos_are_caught() {
        let one = parse(r#"{ "name": "Cave Troll", "hit_points": 84 }"#).unwrap();
        assert_eq!(one[0].name, "Cave Troll");
        let many = parse(r#"[{ "name": "A" }, { "name": "B", "saving_throws": {} }]"#).unwrap();
        assert_eq!(many.len(), 2);

        let err = parse(r#"{ "name": "Troll", "armour_class": 15 }"#).unwrap_err();
        assert!(
            format!("{err:#}").contains("Troll: unknown field(s): armour_class"),
            "{err:#}"
        );
        let err = parse(r#"[{ "name": "A" }, { "name": "" }]"#).unwrap_err();
        assert!(format!("{err:#}").contains("monster 2"), "{err:#}");
        let err = parse(r#"{ "name": "Blob", "hit_dice": "lots" }"#).unwrap_err();
        assert!(format!("{err:#}").contains("hit dice"), "{err:#}");
    }
}
