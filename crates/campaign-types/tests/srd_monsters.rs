//! The SRD monsters from `make srd` (tools/fetch-srd-monsters.sh) must load
//! as stat blocks. They aren't in the repository, so these run on request,
//! after fetching them: `cargo test -p campaign-types -- --ignored`.

use std::path::Path;

use campaign_types::{Edition, monster::MonsterSheet};

fn load(file: &str) -> Vec<MonsterSheet> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/monsters")
        .join(file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run `make srd` first", path.display()));
    serde_json::from_str(&text).expect("the file is a list of stat blocks")
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
