//! Monsters: full stat blocks, kept in a shared library (the SRD) and per
//! campaign (homebrew), and turned into combatants when they join a fight.
//!
//! A stat block is plain JSON, like a character sheet, so monsters can be
//! written by hand or converted from elsewhere (see `tools/`).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Ability, AbilityScores, Edition, Skill};

/// A stored monster: server-assigned metadata plus the stat block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Monster {
    pub id: Uuid,
    /// The campaign whose homebrew it is; `None` for the shared library,
    /// which every campaign (of a matching edition) sees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<Uuid>,
    #[serde(flatten)]
    pub sheet: MonsterSheet,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// For listing and completing names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonsterSummary {
    pub id: Uuid,
    pub name: String,
    pub size: String,
    pub kind: String,
    pub challenge: String,
    pub hit_points: u16,
    /// What its hit dice can come to at most, which it joins a fight with.
    pub max_hit_points: u16,
    pub armor_class: u8,
    pub source: String,
    /// In the shared library rather than the campaign's own.
    pub shared: bool,
}

impl From<&Monster> for MonsterSummary {
    fn from(m: &Monster) -> Self {
        let s = &m.sheet;
        Self {
            id: m.id,
            name: s.name.clone(),
            size: s.size.clone(),
            kind: s.kind.clone(),
            challenge: s.challenge.clone(),
            hit_points: s.hit_points,
            max_hit_points: s.max_hit_points(),
            armor_class: s.armor_class,
            source: s.source.clone(),
            shared: m.campaign.is_none(),
        }
    }
}

/// A full stat block. Everything but the name has a default, so a file
/// needs only what the monster actually has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonsterSheet {
    pub name: String,
    /// "Medium", "Large"...
    pub size: String,
    /// The creature type: "humanoid (goblinoid)", "dragon"...
    #[serde(rename = "type")]
    pub kind: String,
    pub alignment: String,
    pub armor_class: u8,
    /// What the AC comes from: "leather armor, shield".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub armor: String,
    /// The average, as printed.
    pub hit_points: u16,
    /// The hit dice, e.g. "2d6" or "18d10+36"; empty if there are none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hit_dice: String,
    /// "30 ft., fly 60 ft."
    pub speed: String,
    pub ability_scores: AbilityScores,
    /// Overrides the Dexterity modifier (2024 stat blocks print it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiative: Option<i8>,
    /// Saving throw bonuses that differ from the plain modifier.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub saving_throws: BTreeMap<Ability, i8>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub skills: BTreeMap<Skill, i8>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub damage_vulnerabilities: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub damage_resistances: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub damage_immunities: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub condition_immunities: Vec<String>,
    /// "darkvision 60 ft., passive Perception 9"
    pub senses: String,
    pub languages: String,
    /// The challenge rating as printed: "0", "1/8", "1/4", "1/2", "1"...
    pub challenge: String,
    pub xp: u32,
    pub proficiency_bonus: i8,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<Feature>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Feature>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bonus_actions: Vec<Feature>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Feature>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legendary_actions: Vec<Feature>,
    /// Where it's from: "SRD 5.1", "SRD 5.2", "Homebrew"...
    pub source: String,
    /// The rules it's written for; `None` suits either.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edition: Option<Edition>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl Default for MonsterSheet {
    fn default() -> Self {
        Self {
            name: String::new(),
            size: "Medium".into(),
            kind: String::new(),
            alignment: String::new(),
            armor_class: 10,
            armor: String::new(),
            hit_points: 1,
            hit_dice: String::new(),
            speed: "30 ft.".into(),
            ability_scores: AbilityScores::default(),
            initiative: None,
            saving_throws: BTreeMap::new(),
            skills: BTreeMap::new(),
            damage_vulnerabilities: Vec::new(),
            damage_resistances: Vec::new(),
            damage_immunities: Vec::new(),
            condition_immunities: Vec::new(),
            senses: String::new(),
            languages: String::new(),
            challenge: "0".into(),
            xp: 0,
            proficiency_bonus: 2,
            traits: Vec::new(),
            actions: Vec::new(),
            bonus_actions: Vec::new(),
            reactions: Vec::new(),
            legendary_actions: Vec::new(),
            source: "Homebrew".into(),
            edition: None,
            notes: String::new(),
        }
    }
}

/// A trait, action, reaction or legendary action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MonsterError {
    #[error("name must not be empty")]
    EmptyName,
    #[error("hit points must be at least 1")]
    NoHitPoints,
    #[error("hit dice must look like 2d6 or 18d10+36, not {0:?}")]
    InvalidHitDice(String),
    #[error("ability scores must be between 1 and 30")]
    AbilityScoreOutOfRange,
}

/// Hit dice: `count`d`sides` plus `bonus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitDice {
    pub count: u16,
    pub sides: u16,
    pub bonus: i32,
}

impl HitDice {
    /// Parses "2d6", "18d10+36", "4d8 - 4" (spaces allowed).
    pub fn parse(text: &str) -> Option<Self> {
        let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let (count, rest) = text.split_once(['d', 'D'])?;
        let (sides, bonus) = match rest.find(['+', '-']) {
            Some(i) => (&rest[..i], rest[i..].parse().ok()?),
            None => (rest, 0),
        };
        let dice = Self {
            count: count.parse().ok()?,
            sides: sides.parse().ok()?,
            bonus,
        };
        (dice.count > 0 && dice.sides > 0).then_some(dice)
    }

    /// The most the dice can come to (at least 1).
    pub fn max(self) -> u16 {
        let total = i32::from(self.count) * i32::from(self.sides) + self.bonus;
        clamp_hp(total)
    }

    /// Rolls them, with `die` giving each roll (from 1 to `sides`).
    pub fn roll(self, mut die: impl FnMut(u16) -> u16) -> u16 {
        let rolled: i32 = (0..self.count).map(|_| i32::from(die(self.sides))).sum();
        clamp_hp(rolled + self.bonus)
    }
}

fn clamp_hp(total: i32) -> u16 {
    u16::try_from(total.max(1)).unwrap_or(u16::MAX)
}

impl MonsterSheet {
    pub fn validate(&self) -> Result<(), MonsterError> {
        if self.name.trim().is_empty() {
            return Err(MonsterError::EmptyName);
        }
        if self.hit_points == 0 {
            return Err(MonsterError::NoHitPoints);
        }
        if !self.hit_dice.trim().is_empty() && HitDice::parse(&self.hit_dice).is_none() {
            return Err(MonsterError::InvalidHitDice(self.hit_dice.clone()));
        }
        let in_range = |(_, score): (Ability, u8)| (1..=30).contains(&score);
        if !self.ability_scores.iter().all(in_range) {
            return Err(MonsterError::AbilityScoreOutOfRange);
        }
        Ok(())
    }

    pub fn hit_dice(&self) -> Option<HitDice> {
        HitDice::parse(&self.hit_dice)
    }

    /// The most hit points it can have: its hit dice's maximum, or the
    /// printed number if it has no hit dice.
    pub fn max_hit_points(&self) -> u16 {
        self.hit_dice().map_or(self.hit_points, HitDice::max)
    }

    pub fn initiative_modifier(&self) -> i8 {
        self.initiative
            .unwrap_or_else(|| self.ability_scores.modifier(Ability::Dexterity))
    }

    /// The challenge rating as a number, for sorting: "1/4" is 0.25.
    pub fn challenge_value(&self) -> f32 {
        challenge_value(&self.challenge)
    }

    /// Whether a campaign playing by `edition` should see it.
    pub fn suits(&self, edition: Edition) -> bool {
        self.edition.is_none_or(|e| e == edition)
    }
}

/// "1/4" is 0.25, "5" is 5; anything else sorts first.
pub fn challenge_value(challenge: &str) -> f32 {
    match challenge.trim().split_once('/') {
        Some((n, d)) => match (n.parse::<f32>(), d.parse::<f32>()) {
            (Ok(n), Ok(d)) if d != 0.0 => n / d,
            _ => -1.0,
        },
        None => challenge.trim().parse().unwrap_or(-1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_dice_parse_and_give_their_maximum() {
        let dice = HitDice::parse("18d10 + 36").unwrap();
        assert_eq!(
            dice,
            HitDice {
                count: 18,
                sides: 10,
                bonus: 36
            }
        );
        assert_eq!(dice.max(), 216);
        assert_eq!(HitDice::parse("2d6").unwrap().max(), 12);
        assert_eq!(HitDice::parse("1d4-5").unwrap().max(), 1, "never below 1");
        for bad in ["", "d6", "2d", "2x6", "0d6", "2d6+x"] {
            assert_eq!(HitDice::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn rolling_uses_each_die_and_the_bonus() {
        let dice = HitDice::parse("3d8+2").unwrap();
        let mut rolls = vec![1, 8, 4].into_iter();
        assert_eq!(
            dice.roll(|sides| {
                assert_eq!(sides, 8);
                rolls.next().unwrap()
            }),
            15
        );
    }

    #[test]
    fn a_monster_needs_a_name_hit_points_and_sane_dice() {
        let goblin = MonsterSheet {
            name: "Goblin".into(),
            hit_points: 7,
            hit_dice: "2d6".into(),
            ..Default::default()
        };
        assert_eq!(goblin.validate(), Ok(()));
        assert_eq!(goblin.max_hit_points(), 12);
        let no_dice = MonsterSheet {
            hit_dice: String::new(),
            ..goblin.clone()
        };
        assert_eq!(no_dice.max_hit_points(), 7, "the printed number");
        let bad = MonsterSheet {
            hit_dice: "two d six".into(),
            ..goblin.clone()
        };
        assert!(matches!(
            bad.validate(),
            Err(MonsterError::InvalidHitDice(_))
        ));
        assert_eq!(
            MonsterSheet::default().validate(),
            Err(MonsterError::EmptyName)
        );
    }

    #[test]
    fn json_needs_only_what_the_monster_has() {
        let goblin: MonsterSheet = serde_json::from_str(
            r#"{
                "name": "Goblin", "type": "humanoid (goblinoid)", "size": "Small",
                "armor_class": 15, "hit_points": 7, "hit_dice": "2d6",
                "ability_scores": { "strength": 8, "dexterity": 14 },
                "skills": { "stealth": 6 },
                "challenge": "1/4",
                "actions": [{ "name": "Scimitar", "description": "+4 to hit" }]
            }"#,
        )
        .unwrap();
        assert_eq!(goblin.kind, "humanoid (goblinoid)");
        assert_eq!(goblin.skills[&Skill::Stealth], 6);
        assert_eq!(goblin.initiative_modifier(), 2);
        assert_eq!(goblin.challenge_value(), 0.25);
        assert_eq!(goblin.source, "Homebrew");
        assert!(goblin.suits(Edition::Rules2014) && goblin.suits(Edition::Rules2024));
        let back: MonsterSheet =
            serde_json::from_str(&serde_json::to_string(&goblin).unwrap()).unwrap();
        assert_eq!(back, goblin);
    }
}
