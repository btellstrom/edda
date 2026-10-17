//! `edda spell ...` and `condition ...`: browsing the full text of
//! spells and conditions, and keeping the campaign's own.

use std::path::Path;

use anyhow::{Context, Result, bail};
use edda_client::monsters::MonsterClient;
use edda_types::rules_text::{ConditionText, Entry, RulesText, SpellText};
use ratatui::text::Line;
use serde_json::Value;

use crate::{
    cli::TextCommand,
    display::{self, Detail},
    editor::{self, confirm},
    styled,
};

/// Rules text the command line can list and show.
pub trait Shown: RulesText {
    fn lines(&self, width: usize, detail: Detail) -> Vec<Line<'static>>;
    fn list(entries: &[Entry<Self>]) -> String;
}

impl Shown for SpellText {
    fn lines(&self, width: usize, detail: Detail) -> Vec<Line<'static>> {
        display::spell(self, width, detail)
    }

    fn list(entries: &[Entry<Self>]) -> String {
        display::spell_list(entries)
    }
}

impl Shown for ConditionText {
    fn lines(&self, width: usize, detail: Detail) -> Vec<Line<'static>> {
        display::condition(self, width, detail)
    }

    fn list(entries: &[Entry<Self>]) -> String {
        display::text_list(entries)
    }
}

pub async fn run<T: Shown>(client: &MonsterClient, command: TextCommand) -> Result<()> {
    match command {
        TextCommand::List(search) => {
            println!("{}", T::list(&client.texts::<T>(search.as_deref()).await?));
        }
        TextCommand::Show(name) => {
            let entry = find::<T>(client, &name).await?;
            styled::print(&entry.text.lines(styled::print_width(), Detail::Full));
        }
        TextCommand::Add(file) => {
            for text in read::<T>(&file)? {
                let added = client.create_text(&text).await?;
                println!("Added {} to the campaign", added.text.name());
            }
        }
        TextCommand::Export(name) => {
            let entry = find::<T>(client, &name).await?;
            println!("{}", serde_json::to_string_pretty(&entry.text)?);
        }
        TextCommand::Delete { name, yes } => {
            let entry = find::<T>(client, &name).await?;
            let name = entry.text.name();
            if entry.campaign.is_none() {
                bail!("{name} is in the shared library, not the campaign's own");
            }
            if !yes && !confirm(&format!("Delete {name} from the campaign?"))? {
                bail!("not deleted (pass --yes to skip the prompt)");
            }
            client.delete_text::<T>(entry.id).await?;
            println!("Deleted {name}");
        }
    }
    Ok(())
}

/// The one `query` names: its name (any case; the campaign's own before a
/// shared one), or a unique prefix of it.
async fn find<T: RulesText>(client: &MonsterClient, query: &str) -> Result<Entry<T>> {
    let entries = client.texts::<T>(None).await?;
    if let Some(found) = edda_types::rules_text::named(&entries, query) {
        return Ok(found.clone());
    }
    let lower = query.to_lowercase();
    let mut matching: Vec<&Entry<T>> = entries
        .iter()
        .filter(|e| e.text.name().to_lowercase().starts_with(&lower))
        .collect();
    // The same name twice (the campaign's own and a shared one) is one.
    matching.dedup_by(|a, b| a.text.name().eq_ignore_ascii_case(b.text.name()));
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!(
            "no {} called {query:?}; `edda {} list` shows them",
            T::ONE,
            T::ONE
        ),
        many if many.len() <= 8 => {
            let names: Vec<_> = many.iter().map(|e| e.text.name()).collect();
            bail!("{query:?} could be {}", names.join(", "))
        }
        many => bail!(
            "{query:?} matches {} {}; type more of the name",
            many.len(),
            T::KIND
        ),
    }
}

/// Entries from a file: one, or a list; unknown fields are refused so a
/// typo isn't silently dropped, and each is checked.
fn read<T: RulesText>(path: &Path) -> Result<Vec<T>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let input: Value = serde_json::from_str(&text)
        .context("invalid JSON")
        .with_context(|| format!("in {}", path.display()))?;
    let items = match input {
        Value::Array(items) => items,
        single => vec![single],
    };
    items
        .into_iter()
        .enumerate()
        .map(|(n, item)| {
            let label = format!("{} {}", T::ONE, n + 1);
            let text: T = serde_json::from_value(item.clone()).with_context(|| label.clone())?;
            let unknown = editor::unknown_keys(&item, &serde_json::to_value(&text)?, "");
            if !unknown.is_empty() {
                bail!("{label}: unknown field(s): {}", unknown.join(", "));
            }
            text.validate().with_context(|| label)?;
            Ok(text)
        })
        .collect::<Result<_>>()
        .with_context(|| format!("in {}", path.display()))
}
