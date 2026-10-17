//! Lenient parsing of user input into domain types.

use anyhow::{Result, anyhow, bail};
use edda_types::{Ability, CharacterSummary, ClassLevel, Item, Resource, Skill, Spell};
use serde::de::DeserializeOwned;
use uuid::Uuid;

/// Accepts `str`, `strength`, `STR`...
pub fn ability(input: &str) -> Result<Ability, String> {
    let normalized = input.trim().to_ascii_lowercase();
    Ability::ALL
        .into_iter()
        .find(|a| {
            let full = format!("{a:?}").to_ascii_lowercase();
            normalized == full || normalized == full[..3]
        })
        .ok_or_else(|| format!("unknown ability '{input}' (use str, dex, con, int, wis or cha)"))
}

/// Accepts `sleight-of-hand`, `sleight of hand`, `Sleight_Of_Hand`...
pub fn skill(input: &str) -> Result<Skill> {
    from_words(input).map_err(|_| anyhow!("unknown skill '{input}'"))
}

fn from_words<T: DeserializeOwned>(input: &str) -> serde_json::Result<T> {
    let snake: String = input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .collect();
    serde_json::from_value(serde_json::Value::String(snake))
}

/// Sheet entries looked up by name on the command line.
pub trait Named {
    fn name(&self) -> &str;
}

macro_rules! named {
    ($($t:ty),*) => {$(
        impl Named for $t {
            fn name(&self) -> &str {
                &self.name
            }
        }
    )*};
}
named!(ClassLevel, Item, Resource, Spell);

/// Finds the one entry whose name matches `name` case-insensitively.
pub fn by_name<'a, T: Named>(items: &'a mut [T], name: &str, kind: &str) -> Result<&'a mut T> {
    items
        .iter_mut()
        .find(|item| item.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow!("no {kind} named '{name}'"))
}

/// Resolves what the user typed to a single character: an exact name, else a
/// unique name prefix or ID prefix (like a short git hash).
pub fn resolve_character(query: &str, characters: &[CharacterSummary]) -> Result<Uuid> {
    let exact: Vec<_> = characters
        .iter()
        .filter(|c| c.name.eq_ignore_ascii_case(query))
        .collect();
    let candidates = if exact.is_empty() {
        let lower = query.to_lowercase();
        characters
            .iter()
            .filter(|c| {
                c.name.to_lowercase().starts_with(&lower) || c.id.to_string().starts_with(&lower)
            })
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => Ok(one.id),
        // Any word that isn't a command ends up here, so say both.
        [] => bail!(
            "'{query}' isn't a command or a character \
             (`edda --help` lists the commands, `edda list` the characters)"
        ),
        many => {
            let names: Vec<_> = many
                .iter()
                .map(|c| format!("  {} {}", short_id(c.id), c.name))
                .collect();
            bail!(
                "'{query}' matches several characters; be more specific or use an ID:\n{}",
                names.join("\n")
            )
        }
    }
}

pub fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, id: &str) -> CharacterSummary {
        CharacterSummary {
            id: id.parse().unwrap(),
            name: name.into(),
            player_name: None,
            race: String::new(),
            level: 1,
            classes: vec![],
            owners: Vec::new(),
            updated_at: chrono::DateTime::default(),
        }
    }

    fn party() -> Vec<CharacterSummary> {
        vec![
            summary("Grog", "aaaaaaaa-0000-4000-8000-000000000001"),
            summary("Grog Junior", "bbbbbbbb-0000-4000-8000-000000000002"),
            summary("Pike", "abbbbbbb-0000-4000-8000-000000000003"),
        ]
    }

    #[test]
    fn exact_name_beats_prefix() {
        let id = resolve_character("grog", &party()).unwrap();
        assert_eq!(short_id(id), "aaaaaaaa");
    }

    #[test]
    fn unique_prefixes_resolve() {
        assert_eq!(
            short_id(resolve_character("pi", &party()).unwrap()),
            "abbbbbbb"
        );
        assert_eq!(
            short_id(resolve_character("grog j", &party()).unwrap()),
            "bbbbbbbb"
        );
        assert_eq!(
            short_id(resolve_character("bbb", &party()).unwrap()),
            "bbbbbbbb"
        );
    }

    #[test]
    fn ambiguous_and_missing_are_errors() {
        let err = resolve_character("gr", &party()).unwrap_err().to_string();
        assert!(err.contains("several"), "{err}");
        assert!(
            resolve_character("a", &party()).is_err(),
            "aaa.. and abb.. ids"
        );
        let err = resolve_character("vex", &party()).unwrap_err().to_string();
        assert!(
            err.contains("'vex' isn't a command or a character"),
            "{err}"
        );
    }

    #[test]
    fn lenient_enum_parsing() {
        assert_eq!(ability("DEX"), Ok(Ability::Dexterity));
        assert_eq!(ability("wisdom"), Ok(Ability::Wisdom));
        assert!(ability("luck").is_err());
        assert_eq!(skill("sleight of hand").unwrap(), Skill::SleightOfHand);
        assert_eq!(skill("Animal-Handling").unwrap(), Skill::AnimalHandling);
        assert!(skill("cooking").is_err());
    }
}
