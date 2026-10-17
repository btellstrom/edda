use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{resources::Resource, spellcasting::Spellcasting};

/// A stored character: server-assigned metadata plus the editable sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Character {
    pub id: Uuid,
    /// The campaign it belongs to; only that campaign's devices can see it.
    pub campaign: Uuid,
    #[serde(flatten)]
    pub sheet: CharacterSheet,
    /// Incremented on every change. Send it back in `If-Match` when replacing
    /// the sheet so stale copies can't overwrite newer data.
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Lightweight view used when listing characters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterSummary {
    pub id: Uuid,
    pub name: String,
    pub player_name: Option<String>,
    pub race: String,
    pub level: u8,
    pub classes: Vec<ClassLevel>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Character> for CharacterSummary {
    fn from(c: &Character) -> Self {
        Self {
            id: c.id,
            name: c.sheet.name.clone(),
            player_name: c.sheet.player_name.clone(),
            race: c.sheet.race.clone(),
            level: c.sheet.total_level(),
            classes: c.sheet.classes.clone(),
            updated_at: c.updated_at,
        }
    }
}

/// The editable contents of a character sheet. This is the request body for
/// both creating and replacing a character.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharacterSheet {
    pub name: String,
    pub player_name: Option<String>,
    pub race: String,
    pub background: String,
    pub alignment: String,
    pub classes: Vec<ClassLevel>,
    pub experience_points: u32,
    pub ability_scores: AbilityScores,
    pub hit_points: HitPoints,
    pub armor_class: u8,
    pub speed: u16,
    pub saving_throw_proficiencies: Vec<Ability>,
    pub skill_proficiencies: Vec<Skill>,
    pub inventory: Vec<Item>,
    pub spellcasting: Spellcasting,
    /// Limited-use class features and abilities (Action Surge, Rage, Ki...).
    pub resources: Vec<Resource>,
    pub notes: String,
}

impl Default for CharacterSheet {
    fn default() -> Self {
        Self {
            name: String::new(),
            player_name: None,
            race: String::new(),
            background: String::new(),
            alignment: String::new(),
            classes: Vec::new(),
            experience_points: 0,
            ability_scores: AbilityScores::default(),
            hit_points: HitPoints::default(),
            armor_class: 10,
            speed: 30,
            saving_throw_proficiencies: Vec::new(),
            skill_proficiencies: Vec::new(),
            inventory: Vec::new(),
            spellcasting: Spellcasting::default(),
            resources: Vec::new(),
            notes: String::new(),
        }
    }
}

impl CharacterSheet {
    pub const MAX_LEVEL: u8 = 20;

    /// Sum of all class levels (at least 1 for display purposes when empty).
    pub fn total_level(&self) -> u8 {
        let sum: u32 = self.classes.iter().map(|c| u32::from(c.level)).sum();
        u8::try_from(sum.clamp(1, u32::from(Self::MAX_LEVEL))).unwrap_or(Self::MAX_LEVEL)
    }

    pub fn proficiency_bonus(&self) -> i8 {
        // A level of 1 to 20 gives +2 to +6.
        2 + i8::try_from((self.total_level() - 1) / 4).unwrap_or(0)
    }

    pub fn saving_throw(&self, ability: Ability) -> i8 {
        let base = self.ability_scores.modifier(ability);
        if self.saving_throw_proficiencies.contains(&ability) {
            base + self.proficiency_bonus()
        } else {
            base
        }
    }

    pub fn skill_bonus(&self, skill: Skill) -> i8 {
        let base = self.ability_scores.modifier(skill.ability());
        if self.skill_proficiencies.contains(&skill) {
            base + self.proficiency_bonus()
        } else {
            base
        }
    }

    pub fn spell_save_dc(&self, ability: Ability) -> i8 {
        8 + self.proficiency_bonus() + self.ability_scores.modifier(ability)
    }

    pub fn spell_attack_bonus(&self, ability: Ability) -> i8 {
        self.proficiency_bonus() + self.ability_scores.modifier(ability)
    }

    /// Validates rules that serde can't express. Shared by server and clients
    /// so frontends can give immediate feedback.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.name.trim().is_empty() {
            return Err(ValidationError::EmptyName);
        }
        for (ability, score) in self.ability_scores.iter() {
            if !(AbilityScores::MIN..=AbilityScores::MAX).contains(&score) {
                return Err(ValidationError::AbilityScoreOutOfRange { ability, score });
            }
        }
        for class in &self.classes {
            if class.name.trim().is_empty() {
                return Err(ValidationError::EmptyClassName);
            }
            if class.level == 0 {
                return Err(ValidationError::ZeroClassLevel(class.name.clone()));
            }
        }
        let total: u32 = self.classes.iter().map(|c| u32::from(c.level)).sum();
        if total > u32::from(Self::MAX_LEVEL) {
            return Err(ValidationError::LevelTooHigh(total));
        }
        if self.hit_points.current > self.hit_points.max {
            return Err(ValidationError::CurrentHpExceedsMax);
        }
        self.spellcasting.validate()?;
        for (i, resource) in self.resources.iter().enumerate() {
            if resource.name.trim().is_empty() {
                return Err(ValidationError::EmptyResourceName);
            }
            if !resource.uses.is_valid() {
                return Err(ValidationError::Overspent(resource.name.clone()));
            }
            let duplicate = self.resources[..i]
                .iter()
                .any(|r| r.name.eq_ignore_ascii_case(&resource.name));
            if duplicate {
                return Err(ValidationError::DuplicateResource(resource.name.clone()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("name must not be empty")]
    EmptyName,
    #[error("{ability:?} score {score} is outside 1..=30")]
    AbilityScoreOutOfRange { ability: Ability, score: u8 },
    #[error("class name must not be empty")]
    EmptyClassName,
    #[error("class {0} must have a level of at least 1")]
    ZeroClassLevel(String),
    #[error("total level {0} exceeds the maximum of 20")]
    LevelTooHigh(u32),
    #[error("current hit points exceed maximum hit points")]
    CurrentHpExceedsMax,
    #[error("spell name must not be empty")]
    EmptySpellName,
    #[error("spell {0} has invalid level {1} (must be 0-9)")]
    InvalidSpellLevel(String, u8),
    #[error("invalid spell slot level {0}")]
    InvalidSlotLevel(u8),
    #[error("resource name must not be empty")]
    EmptyResourceName,
    #[error("duplicate resource: {0}")]
    DuplicateResource(String),
    #[error("{0}: used exceeds max")]
    Overspent(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassLevel {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subclass: Option<String>,
    pub level: u8,
    /// Set for classes that cast spells (e.g. Intelligence for Wizard).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spellcasting_ability: Option<Ability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AbilityScores {
    pub strength: u8,
    pub dexterity: u8,
    pub constitution: u8,
    pub intelligence: u8,
    pub wisdom: u8,
    pub charisma: u8,
}

impl Default for AbilityScores {
    fn default() -> Self {
        Self {
            strength: 10,
            dexterity: 10,
            constitution: 10,
            intelligence: 10,
            wisdom: 10,
            charisma: 10,
        }
    }
}

impl AbilityScores {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 30;

    pub fn get(&self, ability: Ability) -> u8 {
        match ability {
            Ability::Strength => self.strength,
            Ability::Dexterity => self.dexterity,
            Ability::Constitution => self.constitution,
            Ability::Intelligence => self.intelligence,
            Ability::Wisdom => self.wisdom,
            Ability::Charisma => self.charisma,
        }
    }

    pub fn modifier(&self, ability: Ability) -> i8 {
        // Any score (0 to 255) gives -5 to +122.
        i8::try_from((i16::from(self.get(ability)) - 10).div_euclid(2)).unwrap_or(i8::MAX)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Ability, u8)> + '_ {
        Ability::ALL.into_iter().map(|a| (a, self.get(a)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ability {
    Strength,
    Dexterity,
    Constitution,
    Intelligence,
    Wisdom,
    Charisma,
}

impl Ability {
    pub const ALL: [Ability; 6] = [
        Ability::Strength,
        Ability::Dexterity,
        Ability::Constitution,
        Ability::Intelligence,
        Ability::Wisdom,
        Ability::Charisma,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skill {
    Acrobatics,
    AnimalHandling,
    Arcana,
    Athletics,
    Deception,
    History,
    Insight,
    Intimidation,
    Investigation,
    Medicine,
    Nature,
    Perception,
    Performance,
    Persuasion,
    Religion,
    SleightOfHand,
    Stealth,
    Survival,
}

impl Skill {
    pub fn ability(self) -> Ability {
        use Ability::*;
        match self {
            Skill::Athletics => Strength,
            Skill::Acrobatics | Skill::SleightOfHand | Skill::Stealth => Dexterity,
            Skill::Arcana
            | Skill::History
            | Skill::Investigation
            | Skill::Nature
            | Skill::Religion => Intelligence,
            Skill::AnimalHandling
            | Skill::Insight
            | Skill::Medicine
            | Skill::Perception
            | Skill::Survival => Wisdom,
            Skill::Deception | Skill::Intimidation | Skill::Performance | Skill::Persuasion => {
                Charisma
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HitPoints {
    pub max: u16,
    pub current: u16,
    pub temporary: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub name: String,
    #[serde(default = "one")]
    pub quantity: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

fn one() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> CharacterSheet {
        CharacterSheet {
            name: "Vex".into(),
            classes: vec![ClassLevel {
                name: "Ranger".into(),
                subclass: None,
                level: 5,
                spellcasting_ability: Some(Ability::Wisdom),
            }],
            ability_scores: AbilityScores {
                dexterity: 17,
                wisdom: 8,
                ..Default::default()
            },
            hit_points: HitPoints {
                max: 40,
                current: 40,
                temporary: 0,
            },
            saving_throw_proficiencies: vec![Ability::Dexterity],
            skill_proficiencies: vec![Skill::Stealth],
            ..Default::default()
        }
    }

    #[test]
    fn modifiers_round_down() {
        let s = sheet();
        assert_eq!(s.ability_scores.modifier(Ability::Dexterity), 3);
        assert_eq!(s.ability_scores.modifier(Ability::Wisdom), -1);
        assert_eq!(s.ability_scores.modifier(Ability::Strength), 0);
    }

    #[test]
    fn proficiency_scales_with_level() {
        let mut s = sheet();
        assert_eq!(s.proficiency_bonus(), 3);
        assert_eq!(s.skill_bonus(Skill::Stealth), 6);
        assert_eq!(s.saving_throw(Ability::Dexterity), 6);
        assert_eq!(s.spell_save_dc(Ability::Wisdom), 10);
        assert_eq!(s.spell_attack_bonus(Ability::Dexterity), 6);
        s.classes[0].level = 1;
        assert_eq!(s.proficiency_bonus(), 2);
        s.classes[0].level = 20;
        assert_eq!(s.proficiency_bonus(), 6);
    }

    #[test]
    fn validation_rejects_bad_sheets() {
        assert!(sheet().validate().is_ok());

        let mut s = sheet();
        s.name = "  ".into();
        assert_eq!(s.validate(), Err(ValidationError::EmptyName));

        let mut s = sheet();
        s.ability_scores.strength = 0;
        assert!(matches!(
            s.validate(),
            Err(ValidationError::AbilityScoreOutOfRange { .. })
        ));

        let mut s = sheet();
        s.classes.push(ClassLevel {
            name: "Rogue".into(),
            subclass: None,
            level: 16,
            spellcasting_ability: None,
        });
        assert_eq!(s.validate(), Err(ValidationError::LevelTooHigh(21)));

        let mut s = sheet();
        s.resources = serde_json::from_str(
            r#"[{"name":"Action Surge","max":1,"recharge":"short_rest"},{"name":"action surge","max":1}]"#,
        )
        .unwrap();
        assert!(matches!(
            s.validate(),
            Err(ValidationError::DuplicateResource(_))
        ));

        let mut s = sheet();
        s.spellcasting
            .slots
            .insert(1, crate::Pool { max: 2, used: 3 });
        assert!(matches!(s.validate(), Err(ValidationError::Overspent(_))));
    }

    #[test]
    fn character_round_trips_through_json() {
        // Regression: slot levels are map keys, which `#[serde(flatten)]`
        // on Character used to fail to deserialize.
        let mut sheet = sheet();
        sheet.spellcasting.slots.insert(1, crate::Pool::new(4));
        sheet
            .spellcasting
            .slots
            .insert(3, crate::Pool { max: 2, used: 1 });
        let character = Character {
            campaign: Uuid::nil(),
            id: Uuid::nil(),
            sheet,
            version: 7,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&character).unwrap();
        let back: Character = serde_json::from_str(&json).unwrap();
        assert_eq!(back, character);
    }

    #[test]
    fn minimal_json_fills_defaults() {
        let s: CharacterSheet = serde_json::from_str(r#"{"name":"Pike"}"#).unwrap();
        assert_eq!(s.armor_class, 10);
        assert_eq!(s.ability_scores.charisma, 10);
        assert!(s.validate().is_ok());
    }
}
