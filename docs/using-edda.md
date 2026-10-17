# Using edda

`edda` is the command line everyone at the table uses: the DM and every
player, each on their own devices. It edits character sheets, and opens
full-screen views for fights and for planning them.

## Installing

From a clone of the repository, `make install` (or `cargo install --path
crates/edda`). Without one:

```sh
cargo install --locked --git https://github.com/btellstrom/edda edda
```

Tab completion asks edda itself, so besides commands and options it
completes the campaign's characters (and their commands), monsters,
combats, this device's campaigns and file names. Add one line to your
shell's startup file:

```sh
echo 'source <(COMPLETE=bash edda)' >> ~/.bashrc     # bash
echo 'source <(COMPLETE=zsh edda)' >> ~/.zshrc       # zsh
echo 'COMPLETE=fish edda | source' > ~/.config/fish/completions/edda.fish
```

Names come from the servers as you press Tab (giving up after 1.5 s, when
only commands and options complete); names with spaces come escaped
(`goblin\ boss`).

### Where the servers are

By default edda uses servers on the same machine, as `make up` starts
them. Elsewhere, point it at them, one address for all four when they're
behind the HTTPS proxy:

```sh
export EDDA_SERVER=https://dnd.example.com:8443 \
       EDDA_COMBAT_SERVER=https://dnd.example.com:8443 \
       EDDA_MONSTER_SERVER=https://dnd.example.com:8443
```

| Setting | Option | Default | |
| --- | --- | --- | --- |
| `EDDA_SERVER` | `--server` | `http://127.0.0.1:3000` | characters |
| `EDDA_COMBAT_SERVER` | `--combat-server` (on `fight` and `plan`) | `http://127.0.0.1:3001` | combats and plans |
| `EDDA_MONSTER_SERVER` | `--monster-server` | `http://127.0.0.1:3003` | monsters, spells and conditions |
| `EDDA_AUTH_SERVER` | `--auth-server` | `http://127.0.0.1:3002` | logging in, when setting up a campaign |
| `EDDA_CONFIG_DIR` | | `~/.config/edda` | this device's key and campaigns |

A device that joined by pairing logs in through the auth server it paired
with, so it needs no `EDDA_AUTH_SERVER`.

## Joining a campaign

Each device first joins a campaign. The DM's first device uses the setup
code from `make campaign`; every other device pairs with a DM's device,
both at hand, with a code read out from one to the other:

```sh
edda auth setup <CODE> --name "DM laptop"   # the DM, with the setup code
edda auth invite                            # the DM: shows e.g. 7-tavern-dragon
edda auth pair 7-tavern-dragon --name "Vex's laptop"   # the new device
edda auth devices                           # who's in, certificates checked
edda auth revoke "Vex's laptop"             # the DM locks a device out
```

When the servers are behind HTTPS, invite with the address players
should use: `edda auth invite --auth-server https://dnd.example.com:8443`.

Without both devices at hand, there's a manual way: the player sends the
DM the output of `edda auth key`, the DM runs `edda auth approve <KEY>
--name ...` (`--dm` for another DM device), and the player then runs
`edda auth join <CAMPAIGN ID>`.

A device can be in several campaigns, one in use at a time. Setting up or
joining another adds it and switches to it:

```sh
edda auth campaigns          # the campaigns this device is in; * is the one in use
edda auth use oneshot        # switch, by name (a unique prefix will do) or ID prefix
edda auth forget oneshot     # drop it from this device (it stays registered there)
edda auth status             # this device, its campaign and the campaign's id
```

Everything else works on the campaign in use. A fight screen stays with
the campaign it was opened in.

## Characters

Top-level commands manage the roster; anything else is a character (name,
unique name prefix, or ID prefix) followed by a command:

```sh
edda list
edda new Grog --race Goliath --player Travis
edda import grog.json vex.json   # from JSON files: one sheet or a list each

edda grog                       # show the sheet
edda grog set max-hp 50
edda grog set hp -7             # +N heals, -N damages
edda grog class add Barbarian 3 --subclass "Path of the Berserker"
edda grog skill add athletics intimidation
edda grog resource add Rage 3 --recharge long
edda grog item add Javelin -n 4
edda grog edit                  # the whole sheet as JSON, in $EDITOR

edda keyleth class add Druid 5 --casting wis   # gives her a 5th-level caster's slots
edda keyleth spell add Moonbeam --level 2 --concentration
edda keyleth cast moonbeam --at 3
edda grog use rage
edda grog rest short
```

`edda <CHARACTER> help` lists every command.

Spell slots follow from a character's classes under the campaign's
edition, single-class or multiclass, Pact Magic included; they're worked
out again on every change, and slots already spent stay spent. A short
rest restores short-rest features and pact slots; a long rest restores
everything but features recharged by hand, plus all spell slots and hit
points.

`import` takes the same JSON as `edit` (and as `edda <CHARACTER> show
--json` prints, so an exported character imports again, e.g. into another
campaign); unknown fields are refused, and names already taken are
skipped. `data/characters/` is ignored by git, so it's a place to keep
your own.

Edits retry automatically if someone else saved the character in the
meantime, so they merge rather than overwrite; `edit` reports the conflict
instead, since a whole edited document can't be re-applied safely.

## The fight screen

```sh
edda fight --new "Goblin ambush"   # start a combat
edda fight --plan "goblin ambush"  # start one from a plan, monsters and all
edda fight                         # the latest one still running
edda fight goblin                  # one by name or ID prefix
```

A full-screen view of a combat: the initiative order (with HP, AC and the
effects on each combatant), the sheet of whoever is acting, the log, and a
command line. Everyone on the screen has a number, and a line reads
`[WHO] ACTION [ARGS]`, where `WHO` is a number and defaults to whoever has
the turn:

```
add vex                     Vex joins the combat (from their sheet)
add 3 goblin warrior        Goblin Warrior 1, 2 and 3 join at their maximum HP
add goblin boss roll        Goblin Boss joins with rolled hit points
add Bandit 11               anyone else: a name and hit points
roll · start · next         initiative for everyone without; begin; end the turn
damage 3 7                  Vex deals 7 damage to Goblin
damage 2 3 8                Vex deals 8 damage to Grog and Goblin
2 heal 1 5                  Grog heals Vex for 5
cast hunter's mark 3        Vex casts Hunter's Mark on Goblin (concentration)
cast cure wounds 2 at 2     spends Vex's 2nd-level slot
effect Prone 3 for 1        Vex puts Prone on Goblin for 1 round
effect Fire for 3           Vex puts down Fire for 3 rounds (no target needed)
use favored foe             Vex uses Favored Foe (only ever on oneself)
init 3 17 · init 3          set Goblin's initiative, or roll it
show grappled · show 2      Grappled's description, or number 2's sheet, in the side panel
drop hunter's mark · remove 3 · undo · pause · resume · end · quit
```

As you type, the line below says who does what to whom ("Vex deals ?
damage to Goblin — how much?"), the acting combatant and the targets light
up in the initiative order, and the side panel shows whoever acts. Tab
completes the action, spell, feature, effect (the conditions, and effects
already in play), monster or character; typing a name where a combatant
goes completes it to their number.

| Key | |
| --- | --- |
| Tab, ↑/↓ | complete; pick among the suggestions |
| Esc | clear the line (on an empty line: close what `show` opened) |
| PgUp/PgDn | scroll the log |
| Alt-↑/↓ | scroll the side panel |
| Ctrl-C | quit |

**Monsters** come from the monster service (see [Monsters, spells and
conditions](monsters-and-spells.md)); Tab completes their names with their
CR, type and hit points, and their stat block shows when they act. Several
of one monster are numbered on from any already in the fight.

**Effects.** Every action but `use` takes any combatants as targets.
Effects need none, for things in the area like a fire on the floor; one
with a duration lasts that many rounds, counted in turns of whoever put it
down, and the initiative list shows the rounds left.

**The side panel** opens the sheet at the part the line needs:
spellcasting for a cast, features for a use, a monster's actions for
whatever it does. It also describes what the line is about: the spell
being cast, under the caster's slots, as soon as Tab would pick it; the
effect being put down or dropped, with whom it is on, who's concentrating
on it and how long it has left. `show` keeps a description, or a
combatant's sheet, there while the fight goes on.

**Spells and features.** Casting records the cast in the log and, for
concentration spells, puts the spell into play as an effect on its
targets, ending the caster's previous concentration. The slot or use is
spent on the character's sheet, and given back if the cast is refused or
undone. Hit points go back to the sheets too.

**Undo** takes back your latest command, again to go further back. It
keeps what others did since: undoing 7 damage after someone else dealt 2
leaves the 2.

**Connections.** Changes show at once and are confirmed by the server; if
the connection drops, the screen keeps working, queues what you do and
sends it once it has reconnected. `pause` puts a combat aside, say at the
end of a session, to `resume` days later.

## Planning encounters

```sh
edda plan                     # the plan changed last
edda plan "Goblin ambush"     # that one, or a new one by that name
```

A screen for the DM to put fights together ahead of time. It shows the
open plan's monsters, how hard they'd be for the party, the campaign's
plans, and the stat block of the monster you're typing (Alt-↑/↓ scroll
it). Plans are kept in the campaign, so every DM device sees them, and
they can be used again and again:

```
add 4 goblin · add bugbear roll   add monsters (roll: their HP is rolled when the fight starts)
remove 2 · remove 1 2 · count 1 6  take out entry 2, two of entry 1; set entry 1 to six
party grog · party all             leave Grog out (again: back in); balance it for everyone
notes They wait in the trees.      notes for the plan
fight                              start a fresh combat from the plan (it's kept)
new <name> · open <plan> · rename <name> · delete <plan> · quit
```

Difficulty follows the campaign's edition. Under the **2014** rules the
monsters' XP is multiplied for how many there are (×1 for one, up to ×4 for
fifteen or more, one step up for a party under three and down for six or
more) and compared with the party's Easy, Medium, Hard and Deadly
thresholds. Under **2024**'s it's their plain XP against the party's Low,
Moderate and High budgets. Each character counts at their level; a monster
without `xp` in its stat block gets its challenge rating's.

A fight from a plan holds only its monsters, numbered as `add` numbers
them; characters join in the fight screen as usual.

## Who may do what

A player changes only their own character. Whatever happens to anyone
else, the DM or that character's player enters on their own screen.

- **Characters.** A character belongs to the devices that play it: a
  player's device plays what it creates, and the DM chooses who plays the
  rest (`edda <CHARACTER> owners add <DEVICE>`, with the device's name or
  key prefix from `edda auth devices`; a player's phone and laptop can
  both play one). Every device sees every character.
- **Fights.** A player damages, heals and gives temporary hit points only
  to their own combatants, uses only their features, and casts spells and
  puts down effects that name only themselves. The exception is their own
  concentration: Bless on the party or Hunter's Mark on a goblin may name
  anyone, since it only marks whom it's on. Players end their own turn and
  undo their own commands. Everything else (starting a combat, who's in
  it, initiative, pausing and ending) is the DM's. The fight screen says
  so before sending, and the servers check it.
- **Behind the DM's screen.** The party knows each other's hit points,
  but a player sees only how hurt a monster looks (unhurt, hurt, bloodied
  at half or less, down), not its numbers, armor class or stat block.
- **Monsters, spells, conditions and plans** are the DM's to change;
  plans are hidden from players altogether.
