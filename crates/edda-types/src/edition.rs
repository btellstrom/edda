//! Which edition of the rules a campaign plays by, chosen when the campaign
//! is created. Only where the editions differ does it matter (so far: when
//! Paladins and Rangers get spell slots).

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Which rules a campaign plays by: the 2014 or the 2024 ones.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Edition {
    #[serde(rename = "2014")]
    Rules2014,
    /// The default only for campaigns created before editions were
    /// recorded; new campaigns always name one.
    #[default]
    #[serde(rename = "2024")]
    Rules2024,
}

impl fmt::Display for Edition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Edition::Rules2014 => "2014",
            Edition::Rules2024 => "2024",
        })
    }
}

/// An edition other than 2014 or 2024.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown edition \"{0}\"; use 2014 or 2024")]
pub struct UnknownEdition(String);

impl FromStr for Edition {
    type Err = UnknownEdition;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "2014" => Ok(Edition::Rules2014),
            "2024" => Ok(Edition::Rules2024),
            other => Err(UnknownEdition(other.to_string())),
        }
    }
}
