//! Shell completion. The shell asks edda itself what can come
//! next (`clap_complete`'s dynamic completion), so besides commands and
//! options it completes the campaign's characters (and their commands),
//! monsters, combats and this device's campaigns. Set up with, for bash:
//!
//! ```sh
//! echo 'source <(COMPLETE=bash edda)' >> ~/.bashrc
//! ```
//!
//! Names come from the servers, logged in as this device; if they can't be
//! reached, only commands and options complete.

use std::{ffi::OsStr, future::Future, sync::OnceLock, time::Duration};

use clap::{Command, CommandFactory};
use clap_complete::{
    CompleteEnv,
    engine::{CompletionCandidate, SubcommandCandidates},
};
use edda_client::{Client, combat::CombatClient, monsters::MonsterClient};
use edda_types::{
    combat::CombatStatus,
    rules_text::{ConditionText, SpellText},
};

use crate::{
    cli::{self, CharacterArgs, Cli},
    identity::Identity,
};

/// How long completion waits for the servers.
const PATIENCE: Duration = Duration::from_millis(1500);

/// Which shell is asking; `clap_complete` clears `COMPLETE` before the
/// completers run, so it's noted here first.
static SHELL: OnceLock<Option<String>> = OnceLock::new();

/// Answers the shell and exits, when it's the shell asking.
pub fn answer_if_asked() {
    SHELL.get_or_init(|| std::env::var("COMPLETE").ok());
    CompleteEnv::with_factory(command).complete();
}

/// The command line with the campaign's characters in it, each as a
/// command taking the character commands.
fn command() -> Command {
    let mut command = Cli::command();
    let characters = from_servers(|token| async move {
        let client = Client::new(server("EDDA_SERVER", cli::CHARACTER_SERVER));
        client.with_token(&token).list().await.ok()
    });
    for character in characters.iter().flatten() {
        let name = character.name.to_lowercase();
        // A name with a space can't be a command; it's still offered below.
        if !name.contains(' ') {
            command = command.subcommand(
                CharacterArgs::command()
                    .name(name)
                    .about(format!("{} ({})", character.name, character.race)),
            );
        }
    }
    let spaced: Vec<(String, String)> = characters
        .into_iter()
        .flatten()
        .filter(|c| c.name.contains(' '))
        .map(|c| (c.name.to_lowercase(), c.race))
        .collect();
    command.add(SubcommandCandidates::new(move || {
        spaced
            .iter()
            .map(|(name, race)| candidate(name, race))
            .collect()
    }))
}

/// Monster names for `monster show` and the like.
pub fn monsters(current: &OsStr) -> Vec<CompletionCandidate> {
    let monsters = from_servers(|token| async move {
        let client = MonsterClient::new(server("EDDA_MONSTER_SERVER", cli::MONSTER_SERVER));
        client.with_token(&token).list(None).await.ok()
    });
    matching(current, monsters.into_iter().flatten(), |m| {
        (m.name.clone(), format!("CR {} {}", m.challenge, m.kind))
    })
}

/// Spells, for `spell`.
pub fn spells(current: &OsStr) -> Vec<CompletionCandidate> {
    let spells = from_servers(|token| async move {
        let client = MonsterClient::new(server("EDDA_MONSTER_SERVER", cli::MONSTER_SERVER));
        client
            .with_token(&token)
            .texts::<SpellText>(None)
            .await
            .ok()
    });
    matching(current, spells.into_iter().flatten(), |s| {
        (s.text.name.clone(), crate::display::spell_kind(&s.text))
    })
}

/// Conditions, for `condition`.
pub fn conditions(current: &OsStr) -> Vec<CompletionCandidate> {
    let conditions = from_servers(|token| async move {
        let client = MonsterClient::new(server("EDDA_MONSTER_SERVER", cli::MONSTER_SERVER));
        client
            .with_token(&token)
            .texts::<ConditionText>(None)
            .await
            .ok()
    });
    matching(current, conditions.into_iter().flatten(), |c| {
        (c.text.name.clone(), c.text.source.clone())
    })
}

/// Combats that haven't ended, for `fight`.
pub fn combats(current: &OsStr) -> Vec<CompletionCandidate> {
    let combats = from_servers(|token| async move {
        let client = CombatClient::new(server("EDDA_COMBAT_SERVER", cli::COMBAT_SERVER));
        client.with_token(&token).list(None).await.ok()
    });
    let running = combats
        .into_iter()
        .flatten()
        .filter(|c| c.status != CombatStatus::Ended);
    matching(current, running, |c| {
        (c.name.clone(), format!("round {}", c.round))
    })
}

/// Planned encounters, for `plan` and `fight --plan`.
pub fn encounters(current: &OsStr) -> Vec<CompletionCandidate> {
    let encounters = from_servers(|token| async move {
        let client = CombatClient::new(server("EDDA_COMBAT_SERVER", cli::COMBAT_SERVER));
        client.with_token(&token).encounters().await.ok()
    });
    matching(current, encounters.into_iter().flatten(), |e| {
        (
            e.plan.name.clone(),
            format!("{} monsters", e.plan.monster_count()),
        )
    })
}

/// This device's campaigns, for `auth use` and `auth forget` (no server
/// needed).
pub fn campaigns(current: &OsStr) -> Vec<CompletionCandidate> {
    let profiles = Identity::locate()
        .and_then(|identity| identity.profiles())
        .unwrap_or_default();
    matching(current, profiles, |p| {
        (
            p.campaign.name.clone(),
            format!("{} rules", p.campaign.edition),
        )
    })
}

/// Candidates whose name starts with what's typed (any case), as typed in
/// lower case so they match it.
fn matching<T>(
    current: &OsStr,
    items: impl IntoIterator<Item = T>,
    describe: impl Fn(&T) -> (String, String),
) -> Vec<CompletionCandidate> {
    // Bash passes the word as typed, escapes and all.
    let typed = current.to_string_lossy().replace("\\ ", " ").to_lowercase();
    items
        .into_iter()
        .map(|item| describe(&item))
        .filter(|(name, _)| name.to_lowercase().starts_with(&typed))
        .map(|(name, help)| candidate(&name.to_lowercase(), &help))
        .collect()
}

/// A name, its spaces escaped for shells that insert it as is.
fn candidate(name: &str, help: &str) -> CompletionCandidate {
    let shell = SHELL.get().cloned().flatten();
    let escape = matches!(shell.as_deref(), Some("bash" | "zsh"));
    let value = if escape {
        name.replace(' ', "\\ ")
    } else {
        name.to_string()
    };
    CompletionCandidate::new(value).help(Some(help.to_string().into()))
}

fn server(variable: &str, default: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| default.to_string())
}

/// Logs in and runs `fetch` with the token, giving up quietly (with
/// `None`) if anything fails or it takes too long: a Tab press must never
/// hang or print errors.
fn from_servers<T, F, Fut>(fetch: F) -> Option<T>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        let work = async {
            let (_, session) = Identity::locate().ok()?.login().await.ok()?;
            fetch(session.token).await
        };
        tokio::time::timeout(PATIENCE, work).await.ok().flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_match_any_case_and_escape_spaces_for_bash() {
        // Regression: the shell was looked up after clap_complete had
        // cleared it, so bash got names with bare spaces. The shell is
        // remembered once, as `answer_if_asked` does.
        SHELL.get_or_init(|| Some("bash".into()));
        let names = ["Goblin", "Goblin Boss", "Troll"];
        let found = matching(OsStr::new("GOB"), names, |n| (n.to_string(), String::new()));
        let values: Vec<_> = found
            .iter()
            .map(|c| c.get_value().to_string_lossy().into_owned())
            .collect();
        assert_eq!(values, ["goblin", "goblin\\ boss"]);
        let found = matching(OsStr::new("goblin b"), names, |n| {
            (n.to_string(), String::new())
        });
        assert_eq!(
            found[0].get_value(),
            "goblin\\ boss",
            "matched on what was typed"
        );
        // Regression: bash hands over the word still escaped.
        let found = matching(OsStr::new("goblin\\ b"), names, |n| {
            (n.to_string(), String::new())
        });
        assert_eq!(found.len(), 1, "goblin\\ b is goblin b");
    }
}
