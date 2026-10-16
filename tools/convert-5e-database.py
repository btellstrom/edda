#!/usr/bin/env python3
"""Convert 5e-database's SRD monsters into campaign-manager's monster format.

The input is a 5e-SRD-Monsters.json from https://github.com/5e-bits/5e-database
(src/2014/en for SRD 5.1, src/2024/en for SRD 5.2). The output is a JSON
array of stat blocks, ready for `monster-service import`.

    tools/convert-5e-database.py --edition 2024 5e-SRD-Monsters.json > srd-5.2.json

Only the standard library is used. The SRD is licensed under CC BY 4.0; see
data/monsters/README.md for the attribution that goes with the output.
"""

import argparse
import json
import sys
from fractions import Fraction

ABILITIES = {
    "STR": "strength",
    "DEX": "dexterity",
    "CON": "constitution",
    "INT": "intelligence",
    "WIS": "wisdom",
    "CHA": "charisma",
}

SKILLS = {
    "Acrobatics", "Animal Handling", "Arcana", "Athletics", "Deception",
    "History", "Insight", "Intimidation", "Investigation", "Medicine",
    "Nature", "Perception", "Performance", "Persuasion", "Religion",
    "Sleight of Hand", "Stealth", "Survival",
}


def snake(name):
    return name.lower().replace(" ", "_")


def challenge(rating):
    """0.25 -> "1/4", 2 -> "2"."""
    return str(Fraction(rating).limit_denominator(8))


def armor_class(entries):
    """The first AC listed, and what it comes from."""
    first = entries[0]
    kind = first.get("type")
    if kind == "armor":
        armor = ", ".join(a["name"].lower() for a in first.get("armor", []))
    elif kind == "natural":
        armor = "natural armor"
    elif kind in ("spell", "condition"):
        armor = first.get("desc") or first.get("spell", {}).get("name", "").lower()
    else:
        armor = ""
    return first["value"], armor


def speed(speeds):
    parts = []
    for kind in ("walk", "burrow", "climb", "fly", "swim"):
        if kind in speeds:
            text = speeds[kind] if kind == "walk" else f"{kind} {speeds[kind]}"
            if kind == "fly" and speeds.get("hover"):
                text += " (hover)"
            parts.append(text)
    return ", ".join(parts)


def senses(found):
    parts = [
        f"{kind} {found[kind]}"
        for kind in ("blindsight", "darkvision", "tremorsense", "truesight")
        if kind in found
    ]
    parts.append(f"passive Perception {found['passive_perception']}")
    return ", ".join(parts)


def usage(found):
    """How often a feature can be used, as printed after its name."""
    if not found:
        return ""
    kind = found.get("type")
    if kind == "recharge on roll":
        low = found["min_value"]
        return f" (Recharge {low}–6)" if low < 6 else " (Recharge 6)"
    if kind == "per day":
        times = f"{found['times']}/Day"
        if "times_in_lair" in found:
            times += f", or {found['times_in_lair']}/Day in lair"
        return f" ({times})"
    if kind == "recharge after rest":
        return " (Recharges after a Short or Long Rest)"
    return ""


def features(entries):
    return [
        {"name": f["name"] + usage(f.get("usage")), "description": f.get("desc", "")}
        for f in entries or []
    ]


def convert(monster, edition, source):
    ac, armor = armor_class(monster["armor_class"])
    kind = monster["type"]
    if monster.get("subtype"):
        kind += f" ({monster['subtype']})"
    saves, skills = {}, {}
    for p in monster.get("proficiencies", []):
        label, _, what = p["proficiency"]["name"].partition(": ")
        if label == "Saving Throw" and what in ABILITIES:
            saves[ABILITIES[what]] = p["value"]
        elif label == "Skill" and what in SKILLS:
            skills[snake(what)] = p["value"]
    notes = []
    if monster.get("desc"):
        notes.append(monster["desc"])
    if monster.get("gear"):
        notes.append(f"Gear: {monster['gear']}")
    out = {
        "name": monster["name"],
        "size": monster["size"],
        "type": kind,
        "alignment": monster["alignment"],
        "armor_class": ac,
        "armor": armor,
        "hit_points": monster["hit_points"],
        "hit_dice": monster.get("hit_points_roll") or monster.get("hit_dice", ""),
        "speed": speed(monster["speed"]),
        "ability_scores": {a: monster[a] for a in ABILITIES.values()},
        "saving_throws": saves,
        "skills": skills,
        "damage_vulnerabilities": monster.get("damage_vulnerabilities", []),
        "damage_resistances": monster.get("damage_resistances", []),
        "damage_immunities": monster.get("damage_immunities", []),
        "condition_immunities": [c["name"] for c in monster.get("condition_immunities", [])],
        "senses": senses(monster["senses"]),
        "languages": monster.get("languages") or "—",
        "challenge": challenge(monster["challenge_rating"]),
        "xp": monster.get("xp", 0),
        "proficiency_bonus": monster.get("proficiency_bonus", 2),
        "traits": features(monster.get("special_abilities")),
        "actions": features(monster.get("actions")),
        "bonus_actions": features(monster.get("bonus_actions")),
        "reactions": features(monster.get("reactions")),
        "legendary_actions": features(monster.get("legendary_actions")),
        "source": source,
        "edition": edition,
        "notes": "\n\n".join(notes),
    }
    # Leave out what's empty, as a hand-written file would.
    return {k: v for k, v in out.items() if v not in ("", [], {})}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("input", help="5e-SRD-Monsters.json from 5e-database")
    parser.add_argument("--edition", choices=["2014", "2024"], required=True)
    args = parser.parse_args()
    with open(args.input, encoding="utf-8") as f:
        monsters = json.load(f)
    source = {"2014": "SRD 5.1", "2024": "SRD 5.2"}[args.edition]
    converted = [convert(m, args.edition, source) for m in monsters]
    converted.sort(key=lambda m: m["name"])
    json.dump(converted, sys.stdout, indent=1, ensure_ascii=False)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
