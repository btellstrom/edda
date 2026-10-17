//! Naming combatants: in sentences, and when more of a monster join.

use campaign_types::combat::{CombatState, Combatant};

use super::Context;

/// Names for `count` more of a monster: just its name for the only one in
/// the fight, otherwise numbered on from any already there ("Goblin 2").
pub fn monster_names(base: &str, count: u16, state: &CombatState) -> Vec<String> {
    let mut plain = false;
    let mut highest = 0;
    for combatant in &state.combatants {
        if combatant.name == base {
            plain = true;
        } else if let Some(n) = combatant
            .name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix(' '))
            .and_then(|n| n.parse::<u16>().ok())
        {
            highest = highest.max(n);
        }
    }
    if count == 1 && !plain && highest == 0 {
        return vec![base.to_string()];
    }
    let first = highest.max(u16::from(plain)) + 1;
    (first..first + count)
        .map(|n| format!("{base} {n}"))
        .collect()
}

/// The combatants that target words refer to.
pub(super) fn targets<'c>(words: &[&str], ctx: &'c Context) -> Result<Vec<&'c Combatant>, String> {
    words.iter().map(|w| ctx.target(w)).collect()
}

/// The longest name in `names` that `words` start with, and how many words
/// it took.
pub(super) fn longest_name<'n>(
    words: &[&str],
    names: impl Iterator<Item = &'n str>,
) -> Option<(&'n str, usize)> {
    names
        .filter_map(|name| {
            let parts: Vec<&str> = name.split_whitespace().collect();
            let matches = parts.len() <= words.len()
                && parts
                    .iter()
                    .zip(words)
                    .all(|(p, w)| p.eq_ignore_ascii_case(w));
            matches.then_some((name, parts.len()))
        })
        .max_by_key(|(_, used)| *used)
}

/// "Vex", "Vex and Grog", "Vex, Grog and Pike".
pub(super) fn names(combatants: &[&Combatant]) -> String {
    match combatants {
        [] => String::new(),
        [one] => one.name.clone(),
        [init @ .., last] => {
            let init: Vec<_> = init.iter().map(|c| c.name.as_str()).collect();
            format!("{} and {}", init.join(", "), last.name)
        }
    }
}

pub(super) fn verb_s<'a>(combatants: &[&Combatant], one: &'a str, many: &'a str) -> &'a str {
    if combatants.len() == 1 { one } else { many }
}
