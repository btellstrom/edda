use campaign_types::{
    Pool, Resource,
    combat::{CombatEvent, Effect, SpellSlot},
};
use chrono::Utc;

use super::*;

struct World {
    state: CombatState,
    sheets: HashMap<Uuid, Character>,
    roster: Vec<CharacterSummary>,
    bestiary: Vec<MonsterSummary>,
    spells: Vec<SpellEntry>,
    conditions: Vec<ConditionEntry>,
    /// The player device using the screen, if not the DM's.
    player: Option<String>,
}

fn entry<T: rules_text::RulesText>(text: T) -> rules_text::Entry<T> {
    rules_text::Entry {
        id: Uuid::new_v4(),
        campaign: None,
        text,
        version: 1,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn monster(name: &str, max_hit_points: u16, shared: bool) -> MonsterSummary {
    MonsterSummary {
        id: Uuid::new_v4(),
        name: name.into(),
        size: "Small".into(),
        kind: "humanoid".into(),
        challenge: "1/4".into(),
        hit_points: 7,
        max_hit_points,
        xp: 50,
        armor_class: 15,
        source: if shared { "SRD 5.2" } else { "Homebrew" }.into(),
        shared,
    }
}

fn character(sheet: CharacterSheet) -> Character {
    Character {
        id: Uuid::new_v4(),
        campaign: Uuid::nil(),
        owners: vec!["laura".into()],
        sheet,
        version: 1,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

/// Vex (a ranger with spells and a feature) has the turn; then Grog,
/// a goblin and Pike, who's in the campaign but not the fight.
fn world() -> World {
    let mut vex = CharacterSheet {
        name: "Vex".into(),
        ..Default::default()
    };
    let spell = |name: &str, level, concentration| Spell {
        name: name.into(),
        level,
        class: None,
        prepared: true,
        concentration,
        ritual: false,
        notes: String::new(),
    };
    vex.spellcasting.spells = vec![
        spell("Hunter's Mark", 1, true),
        spell("Cure Wounds", 1, false),
        spell("Cure Wounds Mass", 5, false),
    ];
    vex.spellcasting.slots.insert(1, Pool::new(2));
    vex.spellcasting.slots.insert(2, Pool::new(1));
    vex.resources.push(Resource {
        name: "Favored Foe".into(),
        uses: Pool::new(3),
        recharge: Default::default(),
        notes: String::new(),
    });
    let vex = character(vex);
    let pike = character(CharacterSheet {
        name: "Pike".into(),
        ..Default::default()
    });
    let mut vex_c = Combatant::from_character(&vex);
    vex_c.initiative = Some(20);
    let mut grog = Combatant::new("Grog", 50);
    grog.initiative = Some(15);
    let mut goblin = Combatant::new("Goblin", 7);
    goblin.initiative = Some(10);
    goblin.monster_id = Some(Uuid::new_v4());
    let mut state = CombatState::default();
    for e in [
        CombatEvent::CombatantAdded { combatant: vex_c },
        CombatEvent::CombatantAdded { combatant: grog },
        CombatEvent::CombatantAdded { combatant: goblin },
        CombatEvent::Started,
        CombatEvent::RoundStarted { round: 1 },
    ] {
        state.apply(&e);
    }
    let first = state.combatants[0].id;
    state.apply(&CombatEvent::TurnStarted { combatant: first });
    World {
        state,
        roster: vec![(&vex).into(), (&pike).into()],
        sheets: HashMap::from([(vex.id, vex)]),
        bestiary: vec![
            monster("Cave Troll", 90, true),
            monster("Cave Troll", 120, false),
            monster("Goblin", 12, true),
            monster("Goblin Boss", 30, true),
        ],
        spells: vec![entry(rules_text::SpellText {
            name: "Hunter's Mark".into(),
            level: 1,
            ..Default::default()
        })],
        conditions: vec![entry(rules_text::ConditionText {
            name: "Prone".into(),
            ..Default::default()
        })],
        player: None,
    }
}

impl World {
    fn ctx(&self) -> Context<'_> {
        Context {
            state: &self.state,
            sheets: &self.sheets,
            roster: &self.roster,
            bestiary: &self.bestiary,
            spells: &self.spells,
            conditions: &self.conditions,
            player: self.player.as_deref(),
        }
    }

    fn parse(&self, line: &str) -> Parsed {
        parse(line, &self.ctx())
    }

    fn id(&self, n: usize) -> Uuid {
        self.state.combatants[n - 1].id
    }

    fn lines(&self, line: &str) -> Vec<String> {
        suggest(line, &self.ctx())
            .into_iter()
            .map(|s| s.line)
            .collect()
    }
}

#[test]
fn lines_say_what_they_are_about() {
    let w = world();
    let topic = |line: &str| w.parse(line).topic;
    assert_eq!(
        topic("cast hunter's mark 3"),
        Some(Topic::Spell("Hunter's Mark".into()))
    );
    assert_eq!(
        topic("cast hun"),
        Some(Topic::Spell("hun".into())),
        "still typing"
    );
    assert_eq!(topic("cast"), None);
    assert_eq!(
        topic("effect prone 3 for 2"),
        Some(Topic::Effect("prone".into()))
    );
    assert_eq!(topic("show 2"), Some(Topic::Combatant(w.id(2))));
    assert_eq!(topic("damage 3 5"), None);
    let ctx = w.ctx();
    assert!(ctx.describes(&Topic::Spell("hunter's mark".into())));
    assert!(
        ctx.describes(&Topic::Spell("Cure Wounds".into())),
        "on Vex's sheet"
    );
    assert!(!ctx.describes(&Topic::Spell("hun".into())));
    assert!(ctx.describes(&Topic::Effect("Prone".into())));
}

#[test]
fn show_keeps_a_description_of_anything_named() {
    let w = world();
    assert_eq!(
        w.parse("show prone").plan,
        Ok(vec![Step::Show(Topic::Named("prone".into()))])
    );
    assert_eq!(
        w.parse("show 3").plan,
        Ok(vec![Step::Show(Topic::Combatant(w.id(3)))])
    );
    assert_eq!(
        w.parse("show grog").plan,
        Ok(vec![Step::Show(Topic::Named("grog".into()))]),
        "a combatant by name"
    );
    assert!(w.parse("show nothing").plan.is_err());
    assert!(w.parse("show 9").plan.is_err());
    let offered = w.lines("show hu");
    assert!(
        offered.contains(&"show Hunter's Mark ".to_string()),
        "{offered:?}"
    );
    let offered = w.lines("show ");
    assert!(offered.contains(&"show 1 ".to_string()), "combatants too");
    assert!(offered.contains(&"show Prone ".to_string()));
}

#[test]
fn lines_point_at_the_part_of_the_sheet_they_need() {
    let w = world();
    assert_eq!(w.parse("cast hunt").section, Some(Section::Spellcasting));
    assert_eq!(w.parse("use fav").section, Some(Section::Features));
    assert_eq!(w.parse("damage 3 7").section, None, "nothing on the sheet");
    assert_eq!(
        w.parse("3 damage 1 5").section,
        Some(Section::Actions),
        "a monster's"
    );
    assert_eq!(w.parse("next").section, None);
}

#[test]
fn the_turn_holder_acts_unless_someone_else_is_named() {
    let w = world();
    let p = w.parse("damage 3 7");
    assert_eq!(p.sentence, "Vex deals 7 damage to Goblin");
    assert_eq!(p.actor, Some(w.id(1)));
    assert_eq!(
        p.plan,
        Ok(vec![Step::Combat(CombatCommand::Damage {
            combatant: w.id(3),
            amount: 7
        })])
    );
    let p = w.parse("2 dam 3 7");
    assert_eq!(p.sentence, "Grog deals 7 damage to Goblin");
    assert_eq!(p.actor, Some(w.id(2)));
}

#[test]
fn the_sentence_fills_in_as_you_type() {
    let w = world();
    assert_eq!(w.parse("2").sentence, "Grog …");
    let p = w.parse("heal 2");
    assert_eq!(p.sentence, "Vex heals Grog for ?");
    assert_eq!(p.plan, Err("how much? type the amount".into()));
    let p = w.parse("damage 2 3 8");
    assert_eq!(p.sentence, "Vex deals 8 damage to Grog and Goblin");
    assert_eq!(p.plan.unwrap().len(), 2);
    assert_eq!(
        w.parse("damage gob").plan,
        Err("press Tab to pick \"gob\"".into())
    );
    assert_eq!(
        w.parse("damage 9 1").plan,
        Err("there's no number 9".into())
    );
}

#[test]
fn casting_names_the_slot_and_concentration_becomes_an_effect() {
    let w = world();
    let p = w.parse("cast hunter's mark 3");
    assert_eq!(
        p.sentence,
        "Vex casts Hunter's Mark on Goblin (concentration)"
    );
    let steps = p.plan.unwrap();
    assert!(matches!(
        steps.as_slice(),
        [Step::Combat(CombatCommand::CastSpell { caster, spell, slot: Some(SpellSlot::Level(1)), targets, concentration: Some(_) })]
            if *caster == w.id(1) && spell == "Hunter's Mark" && *targets == [w.id(3)]
    ));

    // The longest matching name wins; no effect without concentration.
    let p = w.parse("cast cure wounds mass 2 3");
    assert_eq!(p.sentence, "Vex casts Cure Wounds Mass on Grog and Goblin");
    assert_eq!(p.plan, Err("no level 5 spell slots".into()));
    let steps = w.parse("cast cure wounds 2 at 2").plan.unwrap();
    assert!(matches!(
        &steps[0],
        Step::Combat(CombatCommand::CastSpell {
            slot: Some(SpellSlot::Level(2)),
            concentration: None,
            ..
        })
    ));
    assert_eq!(
        w.parse("2 cast cure wounds").plan,
        Err("Grog has no character sheet".into())
    );
}

#[test]
fn features_are_used_on_oneself_only() {
    let w = world();
    let p = w.parse("use favored foe 2");
    assert_eq!(p.sentence, "Vex uses Favored Foe ×2");
    assert!(matches!(
        &p.plan.unwrap()[0],
        Step::Combat(CombatCommand::UseFeature { combatant, amount: 2, .. }) if *combatant == w.id(1)
    ));
    assert!(w.parse("use favored foe 3 1").plan.is_err());
    assert_eq!(
        w.parse("use favored foe 4").plan,
        Err("Favored Foe has 3 uses remaining, 4 requested".into())
    );
}

#[test]
fn effects_initiative_and_joining() {
    let mut w = world();
    let p = w.parse("effect Prone 3 for 2");
    assert_eq!(p.sentence, "Vex puts Prone on Goblin for 2 rounds");
    assert!(p.plan.is_ok());
    assert_eq!(w.parse("init 3 17").sentence, "Goblin's initiative is 17");
    assert_eq!(
        w.parse("add pike").plan,
        Ok(vec![Step::Join(w.roster[1].id)])
    );
    assert_eq!(w.parse("add vex").plan, Err("Vex is already in it".into()));
    assert_eq!(
        w.parse("add Goblin Archer 12").sentence,
        "Goblin Archer (12 HP) joins the combat"
    );

    let bless = Effect {
        id: Uuid::new_v4(),
        name: "Bless".into(),
        source: None,
        targets: vec![w.id(2)],
        concentration: false,
        expires: None,
    };
    w.state.apply(&CombatEvent::EffectAdded {
        effect: bless.clone(),
    });
    assert_eq!(
        w.parse("drop bless").plan,
        Ok(vec![Step::Combat(CombatCommand::RemoveEffect {
            effect: bless.id
        })])
    );
}

#[test]
fn certainly_refused_commands_are_caught_before_sending() {
    let w = world();
    assert_eq!(
        w.parse("start").plan,
        Err("combat has already started".into())
    );
    assert!(w.parse("next").plan.is_ok());
    assert_eq!(w.parse("next").sentence, "Vex's turn ends");
    assert!(w.parse("undo").plan.is_ok(), "the server decides undos");
}

#[test]
fn suggestions_complete_actions_names_and_numbers() {
    let w = world();
    let all = suggest("", &w.ctx());
    assert_eq!(all.len(), VERBS.len(), "every action, with its usage");
    assert_eq!(w.lines("he"), ["heal "]);
    // A name first picks who acts, by number.
    assert_eq!(w.lines("gr"), ["2 "]);
    assert_eq!(w.lines("2 ca"), ["2 cast "]);
    // Targets complete to their numbers.
    assert_eq!(w.lines("damage gob"), ["damage 3 "]);
    assert_eq!(w.lines("damage ").len(), 3);
    // Multi-word names complete whole.
    assert_eq!(
        w.lines("cast cure"),
        ["cast Cure Wounds ", "cast Cure Wounds Mass "]
    );
    assert_eq!(w.lines("cast hunter's mark ").len(), 3, "then targets");
    assert_eq!(w.lines("use f"), ["use Favored Foe "]);
    assert_eq!(w.lines("add p"), ["add Pike "]);
    assert!(
        !w.lines("add ").contains(&"add Vex ".to_string()),
        "Vex is already in"
    );
}

#[test]
fn adding_monsters_says_who_joins_and_leaves_the_rest_to_the_server() {
    // The fight already has a plain "Goblin" (number 3).
    let w = world();
    let goblin = w.bestiary[2].id;
    let p = w.parse("add 2 goblin");
    assert_eq!(
        p.sentence,
        "Goblin 2 and Goblin 3 join the combat at 12 HP each"
    );
    assert_eq!(
        p.plan,
        Ok(vec![Step::Combat(CombatCommand::AddMonsters {
            monster: goblin,
            count: 2,
            roll: false
        })])
    );
    let p = w.parse("add goblin boss roll");
    assert_eq!(
        p.sentence,
        "Goblin Boss joins the combat with rolled hit points"
    );
    assert!(matches!(
        p.plan.unwrap()[0],
        Step::Combat(CombatCommand::AddMonsters {
            roll: true,
            count: 1,
            ..
        })
    ));
    let own_troll = w.bestiary[1].id;
    assert!(
        matches!(
            w.parse("add cave troll").plan.unwrap()[0],
            Step::Combat(CombatCommand::AddMonsters { monster, .. }) if monster == own_troll
        ),
        "the campaign's own before the shared one"
    );
    assert_eq!(
        w.parse("add 2 dragon").plan,
        Err("no monster called \"dragon\"; Tab lists them".into())
    );
    assert!(
        matches!(
            w.parse("add Goblin 7").plan.unwrap()[0],
            Step::Combat(CombatCommand::AddCombatant { .. })
        ),
        "a name and hit points still adds anyone"
    );

    assert_eq!(
        w.lines("add 3 gob"),
        ["add 3 Goblin ", "add 3 Goblin Boss "]
    );
    assert!(
        w.lines("add 3 p").is_empty(),
        "characters come one at a time"
    );
    assert_eq!(w.lines("add goblin "), ["add goblin roll "]);
}

#[test]
fn effects_need_no_target_and_last_for_rounds_of_whoever_put_them_down() {
    let w = world();
    let p = w.parse("effect Fire for 3");
    assert_eq!(p.sentence, "Vex puts down Fire for 3 rounds");
    let Ok(steps) = p.plan else {
        panic!("{:?}", p.plan)
    };
    assert!(matches!(
        &steps[0],
        Step::Combat(CombatCommand::AddEffect { effect })
            if effect.name == "Fire" && effect.targets.is_empty()
                && effect.rounds == Some(3) && effect.source == Some(w.id(1))
    ));
    let p = w.parse("2 effect Wall of Fire");
    assert_eq!(p.sentence, "Grog puts down Wall of Fire");
    assert!(p.plan.is_ok(), "until removed");
    assert_eq!(
        w.parse("effect Bless 2 for 10").sentence,
        "Vex puts Bless on Grog for 10 rounds"
    );
    assert_eq!(w.lines("effect Fire f"), ["effect Fire for "]);
    assert!(
        w.lines("effect Fire for ").is_empty(),
        "the rounds come next"
    );
}

#[test]
fn effects_complete_their_name_and_every_target_including_the_first() {
    // Regression: combatants were only offered once a first number had
    // been typed, so the first target got no help, and a name typed
    // there became part of the effect's name.
    let mut w = world();
    assert_eq!(w.lines("effect pr"), ["effect Prone "]);
    assert_eq!(suggest("effect ", &w.ctx()).len(), w.conditions.len());
    assert_eq!(
        w.lines("effect Prone "),
        [
            "effect Prone 1 ",
            "effect Prone 2 ",
            "effect Prone 3 ",
            "effect Prone for "
        ]
    );
    assert_eq!(w.lines("effect Prone gob"), ["effect Prone 3 "]);
    assert_eq!(
        w.parse("effect Prone gob").plan,
        Err("press Tab to pick \"gob\" as a target".into())
    );
    assert_eq!(w.lines("effect Prone 3 gr"), ["effect Prone 3 2 "]);

    // Effects already in play are offered as names, also partway.
    w.state.apply(&CombatEvent::EffectAdded {
        effect: Effect {
            id: Uuid::new_v4(),
            name: "Hunter's Mark".into(),
            source: None,
            targets: vec![],
            concentration: false,
            expires: None,
        },
    });
    assert_eq!(w.lines("effect hu"), ["effect Hunter's Mark "]);
    assert_eq!(w.lines("effect hunter's m"), ["effect Hunter's Mark "]);
}

#[test]
fn a_player_is_told_before_sending_what_only_others_may_do() {
    let w = World {
        player: Some("laura".into()),
        ..world()
    };
    // Vex (number 1) is Laura's and has the turn; Grog and the goblin aren't.
    assert!(w.parse("damage 1 3").plan.is_ok());
    assert_eq!(
        w.parse("damage 3 3").plan,
        Err("Goblin isn't yours; the DM or their player changes them".into())
    );
    assert!(w.parse("next").plan.is_ok());
    assert_eq!(w.parse("end").plan, Err("only the DM can do that".into()));
    assert_eq!(
        w.parse("add pike").plan,
        Err("only the DM can do that".into())
    );
    // Her own concentration may be on anyone; a plain effect may not.
    assert!(w.parse("cast hunter's mark 3").plan.is_ok());
    assert!(w.parse("effect prone 3").plan.is_err());
    let other = World {
        player: Some("travis".into()),
        ..world()
    };
    assert_eq!(other.parse("next").plan, Err("it isn't your turn".into()));
}
