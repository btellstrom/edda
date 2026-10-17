//! Structural edits to a sheet. These run inside `Client::modify`, which may
//! re-apply them to a newer copy, so each must express intent ("add 3 levels
//! of Fighter") rather than overwrite the sheet with a pre-computed result.

use anyhow::{Context, Result, bail};
use edda_types::{Ability, CharacterSheet, ClassLevel, Item, Pool, Resource, Spell};

use crate::{
    cli::{ClassCommand, Field, ItemCommand, ProficiencyCommand, ResourceCommand, SpellCommand},
    parse::{self, by_name},
};

pub fn get_field(sheet: &CharacterSheet, field: Field) -> String {
    let scores = &sheet.ability_scores;
    match field {
        Field::Name => sheet.name.clone(),
        Field::Player => sheet.player_name.clone().unwrap_or_default(),
        Field::Race => sheet.race.clone(),
        Field::Background => sheet.background.clone(),
        Field::Alignment => sheet.alignment.clone(),
        Field::Xp => sheet.experience_points.to_string(),
        Field::Ac => sheet.armor_class.to_string(),
        Field::Speed => sheet.speed.to_string(),
        Field::MaxHp => sheet.hit_points.max.to_string(),
        Field::Hp => sheet.hit_points.current.to_string(),
        Field::TempHp => sheet.hit_points.temporary.to_string(),
        Field::Str => scores.strength.to_string(),
        Field::Dex => scores.dexterity.to_string(),
        Field::Con => scores.constitution.to_string(),
        Field::Int => scores.intelligence.to_string(),
        Field::Wis => scores.wisdom.to_string(),
        Field::Cha => scores.charisma.to_string(),
        Field::Notes => sheet.notes.clone(),
    }
}

pub fn set_field(sheet: &mut CharacterSheet, field: Field, value: &str) -> Result<()> {
    let scores = &mut sheet.ability_scores;
    let hp = &mut sheet.hit_points;
    match field {
        Field::Name => sheet.name = value.to_string(),
        Field::Player => {
            sheet.player_name = Some(value.to_string()).filter(|v| !v.trim().is_empty())
        }
        Field::Race => sheet.race = value.to_string(),
        Field::Background => sheet.background = value.to_string(),
        Field::Alignment => sheet.alignment = value.to_string(),
        Field::Xp => sheet.experience_points = number(value)?,
        Field::Ac => sheet.armor_class = number(value)?,
        Field::Speed => sheet.speed = number(value)?,
        Field::MaxHp => {
            let max = number(value)?;
            // A character at full health stays at full health; otherwise
            // just make sure current HP still fits.
            if hp.current == hp.max || hp.current > max {
                hp.current = max;
            }
            hp.max = max;
        }
        Field::Hp => {
            // "+5" heals and "-7" damages, relative to the current value,
            // so retries on a newer copy apply the change rather than a stale
            // absolute number.
            hp.current = match value.as_bytes().first() {
                Some(b'+') => hp.current.saturating_add(number(&value[1..])?).min(hp.max),
                Some(b'-') => hp.current.saturating_sub(number(&value[1..])?),
                _ => number(value)?,
            }
        }
        Field::TempHp => hp.temporary = number(value)?,
        Field::Str => scores.strength = number(value)?,
        Field::Dex => scores.dexterity = number(value)?,
        Field::Con => scores.constitution = number(value)?,
        Field::Int => scores.intelligence = number(value)?,
        Field::Wis => scores.wisdom = number(value)?,
        Field::Cha => scores.charisma = number(value)?,
        Field::Notes => sheet.notes = value.to_string(),
    }
    Ok(())
}

fn number<T: std::str::FromStr>(value: &str) -> Result<T> {
    value
        .trim()
        .parse()
        .ok()
        .with_context(|| format!("'{value}' is not a valid number here"))
}

pub fn class(sheet: &mut CharacterSheet, command: &ClassCommand) -> Result<()> {
    let classes = &mut sheet.classes;
    match command {
        ClassCommand::Add {
            name,
            level,
            subclass,
            casting,
        } => match classes
            .iter_mut()
            .find(|c| c.name.eq_ignore_ascii_case(name))
        {
            Some(existing) => {
                existing.level = existing.level.saturating_add(*level);
                if subclass.is_some() {
                    existing.subclass.clone_from(subclass);
                }
                if casting.is_some() {
                    existing.spellcasting_ability = *casting;
                }
            }
            None => classes.push(ClassLevel {
                name: name.clone(),
                subclass: subclass.clone(),
                level: *level,
                spellcasting_ability: *casting,
            }),
        },
        ClassCommand::Level { name, level } => by_name(classes, name, "class")?.level = *level,
        ClassCommand::Subclass { name, subclass } => by_name(classes, name, "class")?
            .subclass
            .clone_from(subclass),
        ClassCommand::Remove { name } => {
            by_name(classes, name, "class")?;
            classes.retain(|c| !c.name.eq_ignore_ascii_case(name));
        }
    }
    Ok(())
}

pub fn skills(sheet: &mut CharacterSheet, command: &ProficiencyCommand) -> Result<()> {
    let skills = &mut sheet.skill_proficiencies;
    match command {
        ProficiencyCommand::Add { names } => {
            for name in names {
                let skill = parse::skill(name)?;
                if !skills.contains(&skill) {
                    skills.push(skill);
                }
            }
        }
        ProficiencyCommand::Remove { names } => {
            for name in names {
                let skill = parse::skill(name)?;
                skills.retain(|s| *s != skill);
            }
        }
    }
    Ok(())
}

pub fn saves(sheet: &mut CharacterSheet, command: &ProficiencyCommand) -> Result<()> {
    let saves = &mut sheet.saving_throw_proficiencies;
    let (names, add) = match command {
        ProficiencyCommand::Add { names } => (names, true),
        ProficiencyCommand::Remove { names } => (names, false),
    };
    for name in names {
        let ability: Ability = parse::ability(name).map_err(anyhow::Error::msg)?;
        saves.retain(|a| *a != ability);
        if add {
            saves.push(ability);
        }
    }
    // Keep the conventional STR..CHA order regardless of insertion order.
    saves.sort_by_key(|a| Ability::ALL.iter().position(|x| x == a));
    Ok(())
}

pub fn item(sheet: &mut CharacterSheet, command: &ItemCommand) -> Result<()> {
    let inventory = &mut sheet.inventory;
    match command {
        ItemCommand::Add {
            name,
            quantity,
            notes,
        } => match inventory
            .iter_mut()
            .find(|i| i.name.eq_ignore_ascii_case(name))
        {
            Some(existing) => {
                existing.quantity = existing.quantity.saturating_add(*quantity);
                if let Some(notes) = notes {
                    existing.notes.clone_from(notes);
                }
            }
            None => inventory.push(Item {
                name: name.clone(),
                quantity: *quantity,
                notes: notes.clone().unwrap_or_default(),
            }),
        },
        ItemCommand::Remove { name, quantity } => {
            let existing = by_name(inventory, name, "item")?;
            match quantity {
                Some(n) if *n < existing.quantity => existing.quantity -= n,
                _ => inventory.retain(|i| !i.name.eq_ignore_ascii_case(name)),
            }
        }
    }
    Ok(())
}

pub fn spell(sheet: &mut CharacterSheet, command: &SpellCommand) -> Result<()> {
    let spells = &mut sheet.spellcasting.spells;
    match command {
        SpellCommand::Add {
            name,
            level,
            class,
            concentration,
            ritual,
            unprepared,
        } => {
            if spells.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
                bail!("{name} is already on the spell list");
            }
            spells.push(Spell {
                name: name.clone(),
                level: *level,
                class: class.clone(),
                prepared: !unprepared,
                concentration: *concentration,
                ritual: *ritual,
                notes: String::new(),
            });
            spells.sort_by(|a, b| a.level.cmp(&b.level).then(a.name.cmp(&b.name)));
        }
        SpellCommand::Remove { name } => {
            by_name(spells, name, "spell")?;
            spells.retain(|s| !s.name.eq_ignore_ascii_case(name));
        }
        SpellCommand::Prepare { name } => by_name(spells, name, "spell")?.prepared = true,
        SpellCommand::Unprepare { name } => by_name(spells, name, "spell")?.prepared = false,
    }
    Ok(())
}

pub fn resource(sheet: &mut CharacterSheet, command: &ResourceCommand) -> Result<()> {
    let resources = &mut sheet.resources;
    match command {
        ResourceCommand::Add {
            name,
            max,
            recharge,
        } => {
            if resources.iter().any(|r| r.name.eq_ignore_ascii_case(name)) {
                bail!("{name} already exists; use `resource max` to change it");
            }
            resources.push(Resource {
                name: name.clone(),
                uses: Pool::new(*max),
                recharge: (*recharge).into(),
                notes: String::new(),
            });
        }
        ResourceCommand::Max { name, max } => {
            let uses = &mut by_name(resources, name, "resource")?.uses;
            uses.max = *max;
            uses.used = uses.used.min(*max);
        }
        ResourceCommand::Recharge { name, recharge } => {
            by_name(resources, name, "resource")?.recharge = (*recharge).into()
        }
        ResourceCommand::Remove { name } => {
            by_name(resources, name, "resource")?;
            resources.retain(|r| !r.name.eq_ignore_ascii_case(name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::RechargeArg;

    fn sheet() -> CharacterSheet {
        CharacterSheet {
            name: "Grog".into(),
            hit_points: edda_types::HitPoints {
                max: 30,
                current: 30,
                temporary: 0,
            },
            ..Default::default()
        }
    }

    #[test]
    fn max_hp_keeps_full_health_full() {
        let mut s = sheet();
        set_field(&mut s, Field::MaxHp, "50").unwrap();
        assert_eq!((s.hit_points.current, s.hit_points.max), (50, 50));

        s.hit_points.current = 20;
        set_field(&mut s, Field::MaxHp, "60").unwrap();
        assert_eq!(s.hit_points.current, 20, "wounded stays wounded");
        set_field(&mut s, Field::MaxHp, "10").unwrap();
        assert_eq!(s.hit_points.current, 10, "clamped to new max");
    }

    #[test]
    fn hp_accepts_relative_changes() {
        let mut s = sheet();
        set_field(&mut s, Field::Hp, "-7").unwrap();
        assert_eq!(s.hit_points.current, 23);
        set_field(&mut s, Field::Hp, "+100").unwrap();
        assert_eq!(s.hit_points.current, 30, "healing caps at max");
        set_field(&mut s, Field::Hp, "-100").unwrap();
        assert_eq!(s.hit_points.current, 0);
        set_field(&mut s, Field::Hp, "12").unwrap();
        assert_eq!(s.hit_points.current, 12);
        assert!(set_field(&mut s, Field::Hp, "lots").is_err());
    }

    #[test]
    fn classes_stack_and_remove() {
        let mut s = sheet();
        let add = |level| ClassCommand::Add {
            name: "fighter".into(),
            level,
            subclass: None,
            casting: None,
        };
        class(&mut s, &add(2)).unwrap();
        class(&mut s, &add(1)).unwrap();
        assert_eq!(s.classes.len(), 1);
        assert_eq!(s.classes[0].level, 3);
        class(
            &mut s,
            &ClassCommand::Remove {
                name: "FIGHTER".into(),
            },
        )
        .unwrap();
        assert!(s.classes.is_empty());
        assert!(
            class(
                &mut s,
                &ClassCommand::Remove {
                    name: "Wizard".into()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn items_stack_and_partially_remove() {
        let mut s = sheet();
        let add = |n| ItemCommand::Add {
            name: "Javelin".into(),
            quantity: n,
            notes: None,
        };
        item(&mut s, &add(4)).unwrap();
        item(&mut s, &add(2)).unwrap();
        assert_eq!(s.inventory[0].quantity, 6);
        let remove = |n| ItemCommand::Remove {
            name: "javelin".into(),
            quantity: n,
        };
        item(&mut s, &remove(Some(5))).unwrap();
        assert_eq!(s.inventory[0].quantity, 1);
        item(&mut s, &remove(None)).unwrap();
        assert!(s.inventory.is_empty());
    }

    #[test]
    fn proficiencies_are_deduplicated() {
        let mut s = sheet();
        let names = vec![
            "athletics".into(),
            "Athletics".into(),
            "sleight-of-hand".into(),
        ];
        skills(&mut s, &ProficiencyCommand::Add { names }).unwrap();
        assert_eq!(s.skill_proficiencies.len(), 2);

        let names = vec!["con".into(), "str".into(), "STR".into()];
        saves(&mut s, &ProficiencyCommand::Add { names }).unwrap();
        assert_eq!(
            s.saving_throw_proficiencies,
            vec![Ability::Strength, Ability::Constitution]
        );
    }

    #[test]
    fn resources() {
        let mut s = sheet();
        let add = ResourceCommand::Add {
            name: "Rage".into(),
            max: 3,
            recharge: RechargeArg::Long,
        };
        resource(&mut s, &add).unwrap();
        assert!(resource(&mut s, &add).is_err(), "no duplicates");
    }
}
