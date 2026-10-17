//! Naming combatants in sentences.

use edda_types::combat::Combatant;

use super::Context;

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
