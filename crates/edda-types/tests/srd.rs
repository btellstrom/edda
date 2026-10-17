#![allow(clippy::unwrap_used)] // tests panic on failure, which is the point

//! The SRD's monsters, spells and conditions from `make srd`
//! (tools/fetch-srd.sh) must load and be valid. They aren't in the
//! repository, so these run on request, after fetching them:
//! `cargo test -p edda-types -- --ignored`.

use std::path::Path;

use edda_types::{
    Edition,
    monster::MonsterSheet,
    rules_text::{ConditionText, RulesText, SpellText},
};
use serde::de::DeserializeOwned;

fn load_from<T: DeserializeOwned>(kind: &str, file: &str) -> Vec<T> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data")
        .join(kind)
        .join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run `make srd` first", path.display()));
    serde_json::from_str(&text).expect("the file is a list")
}

fn load(file: &str) -> Vec<MonsterSheet> {
    load_from("monsters", file)
}

fn check(monsters: &[MonsterSheet], count: usize, edition: Edition, source: &str) {
    assert_eq!(monsters.len(), count);
    for m in monsters {
        assert_eq!(m.validate(), Ok(()), "{}", m.name);
        assert_eq!(
            (m.edition, m.source.as_str()),
            (Some(edition), source),
            "{}",
            m.name
        );
        let dice = m
            .hit_dice()
            .unwrap_or_else(|| panic!("{} has hit dice", m.name));
        assert!(dice.max() >= m.hit_points, "{}: max below average", m.name);
        assert!(
            m.challenge_value() >= 0.0,
            "{}: CR {:?}",
            m.name,
            m.challenge
        );
        let features = [&m.traits, &m.actions, &m.bonus_actions, &m.reactions];
        assert!(
            features.iter().any(|f| !f.is_empty()),
            "{} can do nothing",
            m.name
        );
    }
}

#[test]
#[ignore = "needs data/monsters from `make srd`"]
fn srd_5_1_monsters_load() {
    let monsters = load("srd-5.1.json");
    check(&monsters, 334, Edition::Rules2014, "SRD 5.1");
    let goblin = monsters.iter().find(|m| m.name == "Goblin").unwrap();
    assert_eq!((goblin.armor_class, goblin.max_hit_points()), (15, 12));
    assert_eq!(goblin.challenge, "1/4");
}

#[test]
#[ignore = "needs data/monsters from `make srd`"]
fn srd_5_2_monsters_load() {
    let monsters = load("srd-5.2.json");
    check(&monsters, 341, Edition::Rules2024, "SRD 5.2");
    let dragon = monsters
        .iter()
        .find(|m| m.name == "Adult Red Dragon")
        .unwrap();
    assert_eq!(dragon.max_hit_points(), 19 * 12 + 133);
}

fn check_texts<T: RulesText>(texts: &[T], count: usize, edition: Edition, source: &str) {
    assert_eq!(texts.len(), count);
    for t in texts {
        assert_eq!(t.validate(), Ok(()), "{}", t.name());
        assert_eq!(
            (t.edition(), t.source()),
            (Some(edition), source),
            "{}",
            t.name()
        );
    }
}

#[test]
#[ignore = "needs data/spells and data/conditions from `make srd`"]
fn srd_spells_and_conditions_load() {
    for (file, edition, source, spells) in [
        ("srd-5.1.json", Edition::Rules2014, "SRD 5.1", 319),
        ("srd-5.2.json", Edition::Rules2024, "SRD 5.2", 339),
    ] {
        let all: Vec<SpellText> = load_from("spells", file);
        check_texts(&all, spells, edition, source);
        assert!(all.iter().all(|s| !s.description.is_empty()), "{file}");
        let fireball = all.iter().find(|s| s.name == "Fireball").unwrap();
        assert_eq!((fireball.level, fireball.concentration), (3, false));
        let conditions: Vec<ConditionText> = load_from("conditions", file);
        check_texts(&conditions, 15, edition, source);
        assert!(conditions.iter().any(|c| c.name == "Grappled"));
    }
}
