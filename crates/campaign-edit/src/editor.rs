//! `campaign-edit <CHARACTER> edit`: the whole sheet in $EDITOR, git-style.

use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::Path,
    process::Command,
};

use anyhow::{Context, Result, bail};
use campaign_client::{Client, ClientError};
use campaign_types::{Character, CharacterSheet};
use serde_json::Value;

pub async fn edit(client: &Client, character: Character) -> Result<Option<Character>> {
    let path = std::env::temp_dir().join(format!("campaign-edit-{}.json", character.id));
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&character.sheet)? + "\n",
    )
    .with_context(|| format!("writing {}", path.display()))?;

    loop {
        open_editor(&path)?;
        let text = std::fs::read_to_string(&path)?;
        let sheet = match parse_sheet(&text) {
            Ok(sheet) => sheet,
            Err(e) => {
                eprintln!("error: {e:#}");
                if confirm("Edit again?")? {
                    continue;
                }
                bail!("aborted; your edits are kept in {}", path.display());
            }
        };
        if sheet == character.sheet {
            std::fs::remove_file(&path).ok();
            return Ok(None);
        }
        return match client
            .replace(character.id, character.version, &sheet)
            .await
        {
            Ok(saved) => {
                std::fs::remove_file(&path).ok();
                Ok(Some(saved))
            }
            // Unlike single-field edits we can't safely re-apply a whole
            // edited document, so surface the conflict instead of retrying.
            Err(ClientError::Stale(current)) => bail!(
                "{} was changed by someone else while you were editing (now v{}).\n\
                 Nothing was saved; your edits are kept in {}.\n\
                 Run `edit` again to start from the latest version.",
                current.sheet.name,
                current.version,
                path.display()
            ),
            Err(ClientError::Rejected(message)) => {
                bail!("{message}\nYour edits are kept in {}", path.display())
            }
            Err(e) => Err(e.into()),
        };
    }
}

/// Parses the edited JSON, rejecting unknown keys so a typo like
/// `"armour_class"` isn't silently dropped, then runs the shared validation.
fn parse_sheet(text: &str) -> Result<CharacterSheet> {
    let input: Value = serde_json::from_str(text).context("invalid JSON")?;
    let sheet: CharacterSheet = serde_json::from_value(input.clone())?;
    let unknown = unknown_keys(&input, &serde_json::to_value(&sheet)?, "");
    if !unknown.is_empty() {
        bail!("unknown field(s): {}", unknown.join(", "));
    }
    sheet.validate()?;
    Ok(sheet)
}

/// Keys present in `input` that didn't survive a round trip through the
/// sheet type. Empty values are ignored since those are legitimately
/// omitted on output (e.g. `"subclass": null`).
fn unknown_keys(input: &Value, parsed: &Value, path: &str) -> Vec<String> {
    match (input, parsed) {
        (Value::Object(i), Value::Object(p)) => i
            .iter()
            .flat_map(|(key, value)| {
                let here = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match p.get(key) {
                    Some(parsed) => unknown_keys(value, parsed, &here),
                    None if is_empty(value) => vec![],
                    None => vec![here],
                }
            })
            .collect(),
        (Value::Array(i), Value::Array(p)) => i
            .iter()
            .zip(p)
            .enumerate()
            .flat_map(|(n, (i, p))| unknown_keys(i, p, &format!("{path}[{n}]")))
            .collect(),
        _ => vec![],
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn open_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into());
    // Through the shell, like git, so EDITOR="code --wait" works.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path)
        .status()
        .with_context(|| format!("could not run editor '{editor}'"))?;
    if !status.success() {
        bail!("editor '{editor}' exited with {status}");
    }
    Ok(())
}

/// Asks a yes/no question on the terminal. Never blocks when not
/// interactive: answers "no".
pub fn confirm(question: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("{question} [y/N] ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typos_are_caught() {
        let err = parse_sheet(r#"{"name":"Grog","armour_class":18}"#).unwrap_err();
        assert!(err.to_string().contains("armour_class"), "{err}");

        let err = parse_sheet(r#"{"name":"Grog","hit_points":{"maximum":5}}"#).unwrap_err();
        assert!(err.to_string().contains("hit_points.maximum"), "{err}");

        let err =
            parse_sheet(r#"{"name":"Grog","inventory":[{"name":"Axe","qty":2}]}"#).unwrap_err();
        assert!(err.to_string().contains("inventory[0].qty"), "{err}");
    }

    #[test]
    fn omitted_empty_values_are_fine() {
        let sheet = parse_sheet(
            r#"{"name":"Grog","player_name":null,"classes":[{"name":"Barbarian","level":3,"subclass":null}]}"#,
        )
        .unwrap();
        assert_eq!(sheet.classes[0].level, 3);
    }

    #[test]
    fn shared_validation_runs() {
        let err = parse_sheet(r#"{"name":""}"#).unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }
}
