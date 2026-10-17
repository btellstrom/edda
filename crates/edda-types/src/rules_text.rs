//! Rules text: spells' and conditions' full descriptions, kept like
//! monsters in a shared library (the SRD) and per campaign (homebrew), and
//! looked up by name, e.g. for a spell on a character sheet or an effect
//! in a fight.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::Edition;

/// Something kept as rules text: what it's called and where it's from.
pub trait RulesText:
    Clone + std::fmt::Debug + PartialEq + Serialize + DeserializeOwned + Send + Sync + 'static
{
    /// What's stored in the `kind` column, and the API's path: `spells`.
    const KIND: &'static str;
    /// One of them, in words: `spell`.
    const ONE: &'static str;
    /// What it's called.
    fn name(&self) -> &str;
    /// Where it's from: "SRD 5.2", "Homebrew".
    fn source(&self) -> &str;
    /// The rules it's written for; `None` suits either.
    fn edition(&self) -> Option<Edition>;
    /// Checks it can be kept.
    fn validate(&self) -> Result<(), RulesTextError>;
}

/// Why a spell or condition can't be kept.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RulesTextError {
    #[error("name must not be empty")]
    EmptyName,
    #[error("a spell's level is 0 (a cantrip) to 9")]
    SpellLevel,
}

/// A stored piece of rules text: server-assigned metadata plus the text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(bound = "T: RulesText")]
pub struct Entry<T: RulesText> {
    pub id: Uuid,
    /// The campaign whose homebrew it is; `None` for the shared library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<Uuid>,
    #[serde(flatten)]
    pub text: T,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The one called `name` (any case), preferring the campaign's own to the
/// shared library's.
pub fn named<'a, T: RulesText>(entries: &'a [Entry<T>], name: &str) -> Option<&'a Entry<T>> {
    let mut matching = entries
        .iter()
        .filter(|e| e.text.name().eq_ignore_ascii_case(name.trim()));
    let first = matching.next()?;
    Some(if first.campaign.is_some() {
        first
    } else {
        matching.find(|e| e.campaign.is_some()).unwrap_or(first)
    })
}

/// A spell as the books describe it. Everything but the name has a
/// default, so a homebrew spell needs only what it has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct SpellText {
    pub name: String,
    /// 0 for a cantrip.
    pub level: u8,
    pub school: String,
    pub casting_time: String,
    pub range: String,
    /// `V, S, M`.
    pub components: String,
    /// What the M stands for.
    pub material: String,
    pub duration: String,
    pub concentration: bool,
    pub ritual: bool,
    pub classes: Vec<String>,
    /// Paragraphs, one per line; `- ` starts a list item and `**…**` is
    /// bold.
    pub description: String,
    /// Casting it with a higher slot (or, for a cantrip, at higher levels).
    pub higher_levels: String,
    pub source: String,
    pub edition: Option<Edition>,
}

impl Default for SpellText {
    fn default() -> Self {
        Self {
            name: String::new(),
            level: 0,
            school: String::new(),
            casting_time: String::new(),
            range: String::new(),
            components: String::new(),
            material: String::new(),
            duration: String::new(),
            concentration: false,
            ritual: false,
            classes: Vec::new(),
            description: String::new(),
            higher_levels: String::new(),
            source: "Homebrew".into(),
            edition: None,
        }
    }
}

impl RulesText for SpellText {
    const KIND: &'static str = "spells";
    const ONE: &'static str = "spell";

    fn name(&self) -> &str {
        &self.name
    }

    fn source(&self) -> &str {
        &self.source
    }

    fn edition(&self) -> Option<Edition> {
        self.edition
    }

    fn validate(&self) -> Result<(), RulesTextError> {
        if self.name.trim().is_empty() {
            return Err(RulesTextError::EmptyName);
        }
        if self.level > 9 {
            return Err(RulesTextError::SpellLevel);
        }
        Ok(())
    }
}

/// A condition, such as Prone, or any other named effect worth describing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct ConditionText {
    pub name: String,
    /// As for [`SpellText::description`].
    pub description: String,
    pub source: String,
    pub edition: Option<Edition>,
}

impl Default for ConditionText {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            source: "Homebrew".into(),
            edition: None,
        }
    }
}

impl RulesText for ConditionText {
    const KIND: &'static str = "conditions";
    const ONE: &'static str = "condition";

    fn name(&self) -> &str {
        &self.name
    }

    fn source(&self) -> &str {
        &self.source
    }

    fn edition(&self) -> Option<Edition> {
        self.edition
    }

    fn validate(&self) -> Result<(), RulesTextError> {
        if self.name.trim().is_empty() {
            return Err(RulesTextError::EmptyName);
        }
        Ok(())
    }
}

/// A stored spell.
pub type SpellEntry = Entry<SpellText>;
/// A stored condition.
pub type ConditionEntry = Entry<ConditionText>;

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, own: bool) -> ConditionEntry {
        Entry {
            id: Uuid::new_v4(),
            campaign: own.then(Uuid::new_v4),
            text: ConditionText {
                name: name.into(),
                ..Default::default()
            },
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn names_are_found_in_any_case_the_campaigns_own_first() {
        let entries = [
            entry("Prone", false),
            entry("Marked", true),
            entry("prone", true),
        ];
        assert_eq!(named(&entries, "PRONE").unwrap().id, entries[2].id);
        assert_eq!(named(&entries, " marked ").unwrap().id, entries[1].id);
        assert!(named(&entries, "Pron").is_none());
    }

    #[test]
    fn a_stored_entry_is_flat_json() {
        let e = entry("Prone", false);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["name"], "Prone");
        assert_eq!(serde_json::from_value::<ConditionEntry>(json).unwrap(), e);
        let spell: SpellText = serde_json::from_str(r#"{"name":"Zap","level":1}"#).unwrap();
        assert_eq!((spell.level, spell.source.as_str()), (1, "Homebrew"));
        assert_eq!(
            SpellText { level: 10, ..spell }.validate(),
            Err(RulesTextError::SpellLevel)
        );
    }
}
