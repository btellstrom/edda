# Monsters, spells and conditions

The monster service keeps full stat blocks, and the text of spells and
conditions, in two kinds: a **shared library** (the SRD), which every
campaign sees the part of that suits its edition, and each campaign's
**own** (homebrew), which only that campaign sees and its DM can change.
The fight screen and the planner use both.

## The SRD

The SRD isn't part of this repository. `make srd` (that is,
`tools/fetch-srd.sh`) downloads it into `data/`:

| | SRD 5.1 (2014 campaigns) | SRD 5.2 (2024 campaigns) |
| --- | --- | --- |
| Monsters | 334 | 341 |
| Spells | 319 | 339 |
| Conditions | 15 | 15 |

Wizards of the Coast publishes the SRD only as PDFs, so the script takes
the structured version from the community-maintained
[5e-database](https://github.com/5e-bits/5e-database), pinned to one
commit and checked against known checksums, and converts it with
`tools/convert-5e-database.py`. The SRD is licensed under CC BY 4.0; the
attribution is in `data/monsters/README.md` and its neighbours.

Compose's seed job loads whatever is in `data/monsters/`, `data/spells/`
and `data/conditions/` into the shared library every time the servers
start, updating ones with the same name, source and edition. So: `make srd`, then `make up`.

A converter from another source only has to write the formats below;
put its output in `data/` and the seed job loads it too.

## Monsters

```sh
edda monster list goblin           # name, CR, type, AC, HP, where from
edda monster show "goblin boss"    # the stat block
edda monster export troll > cave-troll.json   # a starting point
edda monster add cave-troll.json   # the campaign's own: one stat block, or a list
edda monster edit troll            # in $EDITOR; a shared one is saved as the campaign's copy
edda monster delete "cave troll"   # the campaign's own only
```

A stat block is JSON; everything but `name` is optional, and a file can
hold one or a list. Unknown fields are refused, so typos don't vanish:

```json
{
  "name": "Cave Troll", "size": "Large", "type": "giant", "alignment": "chaotic evil",
  "armor_class": 15, "armor": "natural armor",
  "hit_points": 84, "hit_dice": "8d10+40", "speed": "30 ft.",
  "ability_scores": { "strength": 18, "dexterity": 13, "constitution": 20,
                      "intelligence": 7, "wisdom": 9, "charisma": 7 },
  "initiative": 1,
  "saving_throws": { "constitution": 8 }, "skills": { "perception": 2 },
  "damage_resistances": ["bludgeoning"], "condition_immunities": ["Frightened"],
  "senses": "darkvision 60 ft., passive Perception 12", "languages": "Giant",
  "challenge": "5", "xp": 1800, "proficiency_bonus": 3,
  "traits": [{ "name": "Regeneration", "description": "..." }],
  "actions": [{ "name": "Club", "description": "Melee Attack Roll: +7, ..." }],
  "bonus_actions": [], "reactions": [], "legendary_actions": [],
  "source": "Homebrew", "edition": "2024", "notes": ""
}
```

- `initiative` overrides the Dexterity modifier.
- `edition` (`"2014"` or `"2024"`) limits a shared monster to campaigns
  of that edition; left out, it suits both.
- `xp` left out comes from `challenge`.
- A monster joins a fight at the most its `hit_dice` can come to
  (`hit_points` if it has none), or rolled with `add ... roll`.

## Spells and conditions

The fight screen shows a spell's or condition's text as it's cast or put
down. Spells outside the SRD can be added as the campaign's own; until
then the fight screen shows what the character's sheet says about one.

```sh
edda spell list fire               # name, level and school, C for concentration, source
edda spell show fireball           # the description
edda spell export bless > my-spell.json   # a starting point
edda spell add my-spell.json       # the campaign's own: one, or a list
edda spell delete "my spell"       # the campaign's own only
edda condition list                # the same for conditions
edda condition show grappled
```

A spell is JSON; everything but `name` is optional:

```json
{
  "name": "Zephyr Strike", "level": 1, "school": "Transmutation",
  "casting_time": "Bonus Action", "range": "Self", "components": "V",
  "material": "", "duration": "up to 1 minute", "concentration": true,
  "ritual": false, "classes": ["Ranger"],
  "description": "You move like the wind. ...\n- One list item\n- **Bold.** Another",
  "higher_levels": "", "source": "Homebrew", "edition": "2024"
}
```

A condition is a `name`, a `description` and optionally `source` and
`edition`; any named effect worth describing (a homebrew "Marked") can be
one. In descriptions, each line is a paragraph, `- ` starts a list item and
`**…**` is bold.

## Into the shared library

With compose, put the files in `data/monsters/` (or `spells/`,
`conditions/`) and restart, and the seed job loads them into the shared
library. Run natively, `monster-service import` loads files into the
shared library, or into one campaign:

```sh
monster-service import my-monsters.json [--campaign <ID>]
monster-service import --kind spells my-spells.json [--campaign <ID>]
monster-service import --kind conditions my-conditions.json
```
