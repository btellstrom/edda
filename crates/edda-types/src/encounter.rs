//! Planned encounters and how hard they are.
//!
//! A plan names monsters and how many of each, and the characters it's
//! balanced for. Its difficulty follows the campaign's edition:
//!
//! - **2014** (DMG, "Creating a Combat Encounter"): the monsters' XP,
//!   multiplied for how many there are, against each character's
//!   Easy/Medium/Hard/Deadly threshold, summed over the party.
//! - **2024** (DMG, "Combat Encounters"): the monsters' XP, as is, against
//!   each character's Low/Moderate/High budget, summed over the party.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Edition, monster::challenge_value};

/// A stored plan: server-assigned metadata plus the plan itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Encounter {
    pub id: Uuid,
    pub campaign: Uuid,
    #[serde(flatten)]
    pub plan: EncounterPlan,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EncounterPlan {
    pub name: String,
    pub monsters: Vec<PlannedMonster>,
    /// The characters it's balanced for; empty means the whole party.
    pub party: Vec<Uuid>,
    pub notes: String,
}

/// Some number of one monster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedMonster {
    /// In the monster service.
    pub monster: Uuid,
    /// Its name when planned, shown even if it's since been deleted.
    pub name: String,
    pub count: u16,
    /// Experience points each.
    pub xp: u32,
    /// Roll their hit points when the fight starts, instead of the maximum.
    #[serde(default)]
    pub roll: bool,
}

impl EncounterPlan {
    pub fn monster_count(&self) -> u32 {
        self.monsters.iter().map(|m| u32::from(m.count)).sum()
    }

    /// The monsters' experience points, before any multiplier.
    pub fn xp(&self) -> u32 {
        self.monsters
            .iter()
            .map(|m| m.xp.saturating_mul(u32::from(m.count)))
            .sum()
    }
}

/// Experience points for a challenge rating ("1/4", "5"), for stat blocks
/// that don't say; 0 for one that isn't a rating.
pub fn xp_for_challenge(challenge: &str) -> u32 {
    const BY_CR: [u32; 31] = [
        10, 200, 450, 700, 1100, 1800, 2300, 2900, 3900, 5000, 5900, 7200, 8400, 10000, 11500,
        13000, 15000, 18000, 20000, 22000, 25000, 33000, 41000, 50000, 62000, 75000, 90000, 105000,
        120000, 135000, 155000,
    ];
    match challenge_value(challenge) {
        cr if cr < 0.0 => 0,
        cr if cr >= 1.0 => BY_CR.get(whole(cr)).copied().unwrap_or(0),
        cr if cr >= 0.5 => 100,
        cr if cr >= 0.25 => 50,
        cr if cr >= 0.125 => 25,
        _ => 10,
    }
}

/// The whole part of a rating of at least 1.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "ratings are small and positive here"
)]
fn whole(rating: f32) -> usize {
    rating as usize
}

/// How hard an encounter is for a party, under one edition's rules.
#[derive(Debug, Clone, PartialEq)]
pub struct Difficulty {
    pub edition: Edition,
    /// The monsters' XP as compared with the thresholds: multiplied for
    /// their number under the 2014 rules, as is under 2024's.
    pub xp: u32,
    /// For 2014, the multiplier used; 1 under 2024.
    pub multiplier: f32,
    /// The party's thresholds, lowest first, with their names.
    pub thresholds: Vec<(&'static str, u32)>,
    /// The highest threshold reached, if any.
    pub rating: Option<&'static str>,
}

/// 2014: Easy, Medium, Hard and Deadly XP thresholds per character level.
const THRESHOLDS_2014: [[u32; 4]; 20] = [
    [25, 50, 75, 100],
    [50, 100, 150, 200],
    [75, 150, 225, 400],
    [125, 250, 375, 500],
    [250, 500, 750, 1100],
    [300, 600, 900, 1400],
    [350, 750, 1100, 1700],
    [450, 900, 1400, 2100],
    [550, 1100, 1600, 2400],
    [600, 1200, 1900, 2800],
    [800, 1600, 2400, 3600],
    [1000, 2000, 3000, 4500],
    [1100, 2200, 3400, 5100],
    [1250, 2500, 3800, 5700],
    [1400, 2800, 4300, 6400],
    [1600, 3200, 4800, 7200],
    [2000, 3900, 5900, 8800],
    [2100, 4200, 6300, 9500],
    [2400, 4900, 7300, 10900],
    [2800, 5700, 8500, 12700],
];

/// 2024: Low, Moderate and High XP budgets per character level.
const BUDGETS_2024: [[u32; 3]; 20] = [
    [50, 75, 100],
    [100, 150, 200],
    [150, 225, 400],
    [250, 375, 500],
    [500, 750, 1100],
    [600, 1000, 1400],
    [750, 1300, 1700],
    [1000, 1700, 2100],
    [1300, 2000, 2600],
    [1600, 2300, 3100],
    [1900, 2900, 4100],
    [2200, 3700, 4700],
    [2600, 4200, 5400],
    [2900, 4900, 6200],
    [3300, 5400, 7800],
    [3800, 6100, 9800],
    [4500, 7200, 11700],
    [5000, 8700, 14200],
    [5500, 10700, 17200],
    [6400, 13200, 22000],
];

/// 2014's encounter multipliers, by how many monsters (1, 2, 3–6, 7–10,
/// 11–14, 15+), with one step either side for small and large parties.
const MULTIPLIERS: [f32; 8] = [0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0];

fn multiplier(monsters: u32, party: usize) -> f32 {
    let step = match monsters {
        0 | 1 => 1,
        2 => 2,
        3..=6 => 3,
        7..=10 => 4,
        11..=14 => 5,
        _ => 6,
    };
    // Fewer than three characters: the next multiplier up; six or more:
    // the next down.
    let step = match party {
        0..=2 => step + 1,
        3..=5 => step,
        _ => step - 1,
    };
    MULTIPLIERS[step]
}

/// The party's thresholds: each character's, by level, summed.
fn party_totals<const N: usize>(levels: &[u8], table: &[[u32; N]; 20]) -> [u32; N] {
    let mut totals = [0; N];
    for &level in levels {
        let row = table[usize::from(level.clamp(1, 20)) - 1];
        for (total, threshold) in totals.iter_mut().zip(row) {
            *total += threshold;
        }
    }
    totals
}

/// How hard `plan` is for characters of `levels`, under `edition`.
pub fn difficulty(plan: &EncounterPlan, levels: &[u8], edition: Edition) -> Difficulty {
    let (names, totals, multiplier): (&[&'static str], Vec<u32>, f32) = match edition {
        Edition::Rules2014 => (
            &["Easy", "Medium", "Hard", "Deadly"],
            party_totals(levels, &THRESHOLDS_2014).to_vec(),
            multiplier(plan.monster_count(), levels.len()),
        ),
        Edition::Rules2024 => (
            &["Low", "Moderate", "High"],
            party_totals(levels, &BUDGETS_2024).to_vec(),
            1.0,
        ),
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "XP times a multiplier of at most 5 is positive and fits"
    )]
    let xp = (plan.xp() as f32 * multiplier).round() as u32;
    let thresholds: Vec<_> = names.iter().copied().zip(totals).collect();
    let rating = thresholds
        .iter()
        .rev()
        .find(|(_, threshold)| *threshold > 0 && xp >= *threshold)
        .map(|(name, _)| *name);
    Difficulty {
        edition,
        xp,
        multiplier,
        thresholds,
        rating,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(monsters: &[(u16, u32)]) -> EncounterPlan {
        EncounterPlan {
            name: "Test".into(),
            monsters: monsters
                .iter()
                .map(|&(count, xp)| PlannedMonster {
                    monster: Uuid::new_v4(),
                    name: "M".into(),
                    count,
                    xp,
                    roll: false,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn challenge_ratings_give_experience_points() {
        assert_eq!(xp_for_challenge("0"), 10);
        assert_eq!(xp_for_challenge("1/8"), 25);
        assert_eq!(xp_for_challenge("1/4"), 50);
        assert_eq!(xp_for_challenge("1/2"), 100);
        assert_eq!(xp_for_challenge("5"), 1800);
        assert_eq!(xp_for_challenge("30"), 155000);
        assert_eq!(xp_for_challenge("lots"), 0);
    }

    #[test]
    fn rules_2014_multiply_for_numbers_against_the_party_thresholds() {
        // The DMG's example: four 3rd-level characters (Medium 600, Hard
        // 900, Deadly 1600) against a bugbear and three hobgoblins:
        // 200 + 3 * 100 = 500 XP, times 2 for four monsters = 1000: Hard.
        let ambush = plan(&[(1, 200), (3, 100)]);
        let d = difficulty(&ambush, &[3, 3, 3, 3], Edition::Rules2014);
        assert_eq!(
            d.thresholds,
            [
                ("Easy", 300),
                ("Medium", 600),
                ("Hard", 900),
                ("Deadly", 1600)
            ]
        );
        assert_eq!((d.multiplier, d.xp, d.rating), (2.0, 1000, Some("Hard")));
        // Two characters: the next multiplier up.
        let pair = difficulty(&ambush, &[3, 3], Edition::Rules2014);
        assert_eq!((pair.multiplier, pair.xp), (2.5, 1250));
        // Six: the next down; one monster for six is halved.
        let lone = difficulty(&plan(&[(1, 200)]), &[1; 6], Edition::Rules2014);
        assert_eq!((lone.multiplier, lone.xp, lone.rating), (0.5, 100, None));
    }

    #[test]
    fn rules_2024_compare_plain_xp_with_the_party_budget() {
        // Four 5th-level characters: Low 2000, Moderate 3000, High 4400.
        let ogres = plan(&[(2, 1100)]);
        let d = difficulty(&ogres, &[5, 5, 5, 5], Edition::Rules2024);
        assert_eq!(
            d.thresholds,
            [("Low", 2000), ("Moderate", 3000), ("High", 4400)]
        );
        assert_eq!((d.multiplier, d.xp, d.rating), (1.0, 2200, Some("Low")));
        let more = difficulty(&plan(&[(5, 1100)]), &[5, 5, 5, 5], Edition::Rules2024);
        assert_eq!(more.rating, Some("High"));
        let none = difficulty(&ogres, &[], Edition::Rules2024);
        assert_eq!(none.rating, None, "no party, no rating");
    }
}
