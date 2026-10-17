# Campaign Manager

A collection of small Rust services for running a D&D campaign, with state held
server-side and any number of frontends talking to it over HTTP/JSON.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/campaign-types` | Shared domain models, rules helpers (modifiers, proficiency) and validation. No server deps, so frontends (including WASM) can depend on it. |
| `crates/campaign-auth` | Device keys, signed device certificates and tokens. Shared by the services and clients. |
| `crates/campaign-server` | What every service shares: logging, opening its database, the auth service's key, serving (with the CORS setting and the limits) and error responses. |
| `crates/auth-service` | Campaigns, the devices admitted to them, and login (axum + Postgres). |
| `crates/character-service` | Character sheet CRUD API (axum + Postgres). |
| `crates/combat-service` | Live, event-sourced combat encounters (axum + Postgres + WebSocket). Runs as any number of instances. |
| `crates/monster-service` | Monster stat blocks and the text of spells and conditions: a shared library (the SRD) and each campaign's own (axum + Postgres). |
| `crates/campaign-client` | Typed async client for the services, plus `Replica` for optimistic UIs. Shared by all frontends. |
| `crates/campaign-edit` | Git-style command-line frontend for editing character sheets. |

## Running

The servers run as containers; `campaign-edit` (the command line and the
fight screen) is a native program.

```sh
make server       # the servers' container image, from the Dockerfile
make client       # campaign-edit, natively: target/release/campaign-edit
make install      # campaign-edit into ~/.cargo/bin
make srd          # download the SRD monsters, spells and conditions into data/ (optional)
make up           # build and start the servers; make down stops them
make campaign NAME="Vox Machina" EDITION=2024   # the DM's setup code
make delete-campaign ID=<id>     # a campaign and everything in it (asks first; YES=1 doesn't)
make              # lists every target
```

The Makefile uses podman; `make CONTAINER=docker ...` uses Docker.

### Servers in containers

`compose.yaml` runs Postgres, the auth, character, combat and monster
services (all from one image, built by the `Dockerfile`), and two one-off
jobs: one registers the combat service's key so it can write hit points
back to character sheets, the other loads the SRD monsters into the
monster service's shared library (on every start, updating them).

```sh
podman compose up -d --build     # or: docker compose up -d --build
podman compose exec auth auth-service new-campaign "Vox Machina" --edition 2024
podman compose logs -f combat    # logs
podman compose down              # stop; data stays in volumes (down -v deletes it)
```

Deleting a campaign removes it from every service: its devices (which
can no longer log in), characters, combats, planned encounters and its own monsters. `campaign-edit auth
status` shows its id. Without make, run `delete-campaign <ID>` with each
of `auth-service`, `character-service` and `combat-service`; each deletes
its own part and can be re-run safely.

`podman compose` needs podman's API socket: `systemctl --user enable --now
podman.socket`. The services are published on localhost: auth 3002,
characters 3000, combat 3001, monsters 3003, Postgres 5432. Move any of
them with `AUTH_PORT`, `CHARACTERS_PORT`, `COMBAT_PORT`, `MONSTERS_PORT` or
`POSTGRES_PORT`, e.g. if
another Postgres already has 5432:

```sh
POSTGRES_PORT=5433 podman compose up -d
```

#### Players' devices: HTTPS

To let players' devices in, put the stack behind HTTPS rather than
opening the services themselves: the `https` profile adds a proxy
(Caddy, configured in `proxy/Caddyfile`) that serves all four services,
WebSockets included, on one address.

```sh
make up HTTPS=1 CAMPAIGN_HOST=dnd.example.com    # a domain pointing at this machine
make up HTTPS=1 CAMPAIGN_HOST=192.168.1.20       # or its address on your network
```

It listens on 8443 (and 8080, which redirects), since rootless Podman
can't take 443; set `HTTPS_PORT`/`HTTP_PORT` to change that.

- **A domain** gets a certificate from Let's Encrypt automatically: point
  the domain at the machine, and the router's port 443 at `HTTPS_PORT`.
- **A local address** (or `localhost`) gets a certificate from Caddy's
  own authority, which each device must trust once. Fetch its root with
  `podman compose cp proxy:/data/caddy/pki/authorities/local/root.crt .`
  and add it to the system's certificates (Arch: `trust anchor root.crt`;
  Debian: copy it to `/usr/local/share/ca-certificates/` as a `.crt` and
  run `update-ca-certificates`), or for campaign-edit alone set
  `SSL_CERT_FILE=root.crt`.

Then point campaign-edit at it, one address for all four. A device logs
in through the address it joined with, so players pair with it too: on
the DM's device, `campaign-edit auth invite --auth-server
https://dnd.example.com:8443` tells them so.

```sh
export CAMPAIGN_SERVER=https://dnd.example.com:8443 CAMPAIGN_COMBAT_SERVER=https://dnd.example.com:8443 \
       CAMPAIGN_MONSTER_SERVER=https://dnd.example.com:8443
```

`tools/sandbox.sh https` runs the same proxy in front of the sandbox, to
try it: `SANDBOX_HTTPS=1` then sends everything through it.

**Browsers.** The services answer web pages from other sites only if
`CORS_ORIGINS` lists them (comma-separated origins such as
`https://dnd.example.com`, or `*` while developing). Unset, none may:
campaign-edit doesn't need it, and neither does a page served through the
proxy.

#### Limits

A service open to others has to survive clients that ask too much, by
mistake or not. Every service keeps limits, on by default:

| Limit | Default | Setting |
| --- | --- | --- |
| Requests per minute from one client address | 600 | `LIMIT_REQUESTS_PER_MINUTE` |
| ...to the routes that need no token (logging in, setting up a campaign, pairing) | 20 | `LIMIT_SIGN_INS_PER_MINUTE` |
| Size of a request | 1 MiB | `LIMIT_BODY_BYTES` |
| Characters per campaign | 200 | `LIMIT_CHARACTERS` |
| A campaign's own monsters, and its own spells and conditions (each) | 1000 | `LIMIT_MONSTERS`, `LIMIT_RULES_TEXTS` |
| Combats, and planned encounters, per campaign | 500, 200 | `LIMIT_COMBATS`, `LIMIT_PLANS` |
| Devices per campaign | 50 | `LIMIT_DEVICES` |
| Events in one combat | 20,000 | `LIMIT_EVENTS_PER_COMBAT` |
| Devices watching one combat at once | 50 | `LIMIT_CONNECTIONS_PER_COMBAT` |

`LIMITS=off` turns them all off, for your own machine (`LIMITS=off make
up`, or `LIMITS=off` in a `.env` file next to `compose.yaml`); the sandbox
runs without them. A client over its rate gets `429` with `Retry-After`;
a full campaign gets `403` saying which limit it reached.

Requests are counted per service instance and client address. Behind the
proxy, `TRUST_PROXY=1` (set in compose) takes the address from the
`X-Forwarded-For` the proxy adds and counts only requests that came
through it, so the services' calls to each other aren't limited; set it
only where nothing else can reach the services.

#### Backups

```sh
make backup                           # into backups/<date and time>; the servers keep running
make restore FROM=backups/<date>      # replaces all data with the backup's
```

A backup holds the four services' databases (`pg_dump`) and the combat
service's key. Restoring stops the four services, puts it all back and
starts them again. Copy `backups/` somewhere else: a backup on the same
disk is lost with it. The smoke test backs up, loses a character and
restores it.

#### Upgrading from SQLite

The character and monster services used to keep their data in SQLite
files, in their volumes; now everything is in Postgres. After upgrading,
bring it over once:

```sh
make up               # the new services, with empty databases (and the SRD seeded)
make migrate-sqlite   # the characters, then the monsters, spells and conditions
```

Everything comes over as it was, ids included, so plans, combats and
devices that refer to it still do. Each import is all or nothing, and
refuses to run over data that's already there (the seeded shared library
excepted, which the old one replaces), so it can't be run twice by
mistake. The old files stay in the volumes until you delete them.

### Servers natively

The combat and auth services need Postgres. Start one with
`podman compose up -d postgres`, or without a compose provider:

```sh
podman run -d --name campaign-postgres -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:5432:5432 -v campaign-postgres:/var/lib/postgresql/data \
  docker.io/library/postgres:17-alpine
```

Start the auth service first: the others fetch its token key on startup
(or set `AUTH_PUBLIC_KEY` to it, which is better when the network between
them isn't trusted; `AUTH_SERVICE_URL` says where to fetch it from).

```sh
cargo run -p auth-service
# DATABASE_URL defaults to postgres://postgres:postgres@localhost:5432/auth,
#   BIND_ADDR to 127.0.0.1:3002
cargo run -p auth-service -- new-campaign "Vox Machina" --edition 2024   # the DM's setup code

cargo run -p character-service
# DATABASE_URL defaults to postgres://postgres:postgres@localhost:5432/characters,
#   BIND_ADDR to 127.0.0.1:3000
cargo run -p combat-service
# DATABASE_URL defaults to postgres://postgres:postgres@localhost:5432/combats
#   (created on first start), BIND_ADDR to 127.0.0.1:3001,
# CHARACTER_SERVICE_URL to http://127.0.0.1:3000 (empty leaves character sheets alone),
# MONSTER_SERVICE_URL to http://127.0.0.1:3003 (empty: no adding monsters by id)
cargo run -p monster-service -- import data/monsters/*.json   # the SRD, once
cargo run -p monster-service -- import --kind spells data/spells/*.json
cargo run -p monster-service -- import --kind conditions data/conditions/*.json
cargo run -p monster-service
# DATABASE_URL defaults to postgres://postgres:postgres@localhost:5432/monsters,
#   BIND_ADDR to 127.0.0.1:3003
```

On first start the combat service creates its own key (`SERVICE_KEY_FILE`,
default `combat-service.key`) and logs the command that lets it write hit
points back: `auth-service add-service combat-service <KEY>`
(`combat-service public-key` prints the key).

Run more combat-service instances against the same database (with
different `BIND_ADDR`s, behind any load balancer) and clients may connect to
whichever they like.

## Command-line editor

```sh
cargo install --path crates/campaign-edit   # or: cargo run -p campaign-edit -- <args>
export CAMPAIGN_SERVER=http://127.0.0.1:3000  # the default; or pass --server
```

Tab completion asks campaign-edit itself, so besides commands and options
it completes the campaign's characters (and their commands), monsters,
combats, this device's campaigns and file names. Add one line to your
shell's startup file:

```sh
echo 'source <(COMPLETE=bash campaign-edit)' >> ~/.bashrc     # bash
echo 'source <(COMPLETE=zsh campaign-edit)' >> ~/.zshrc       # zsh
echo 'COMPLETE=fish campaign-edit | source' > ~/.config/fish/completions/campaign-edit.fish
```

Names come from the servers as you press Tab (giving up after 1.5 s, when
only commands and options complete); names with spaces come escaped
(`goblin\ boss`).

Each device first joins a campaign (see [Authentication](#authentication)):

```sh
campaign-edit auth setup <CODE> --name "DM laptop"   # the DM, with the setup code
campaign-edit auth invite                            # the DM: shows e.g. 7-tavern-dragon
campaign-edit auth pair 7-tavern-dragon --name "Vex's laptop"   # the new device
campaign-edit auth devices                           # who's in, certificates checked
campaign-edit auth revoke "Vex's laptop"             # the DM locks a device out
```

Without both devices at hand, there's a manual way: the player sends the
DM the output of `auth key`, the DM runs `auth approve <KEY> --name ...`,
and the player then runs `auth join <CAMPAIGN ID>`.

A device can be in several campaigns, one in use at a time. Setting up or
joining another adds it and switches to it:

```sh
campaign-edit auth campaigns          # the campaigns this device is in; * is the one in use
campaign-edit auth use oneshot        # switch, by name (a unique prefix will do) or ID prefix
campaign-edit auth forget oneshot     # drop it from this device (it stays registered there)
```

Everything else (characters, `fight`) works on the campaign in use. A
fight screen stays with the campaign it was opened in. The profiles live in
`~/.config/campaign-manager/campaigns/`, next to the device key, which is
the same in every campaign.

Top-level commands manage the roster; anything else is a character (name,
unique name prefix, or ID prefix) followed by a command:

```sh
campaign-edit list
campaign-edit new Grog --race Goliath --player Travis
campaign-edit import data/characters/*.json   # from JSON files: one sheet or a list each

campaign-edit grog                       # show the sheet
campaign-edit grog set max-hp 50
campaign-edit grog set hp -7             # +N heals, -N damages
campaign-edit grog class add Barbarian 3 --subclass "Path of the Berserker"
campaign-edit grog skill add athletics intimidation
campaign-edit grog resource add Rage 3 --recharge long
campaign-edit grog item add Javelin -n 4
campaign-edit grog edit                  # whole sheet as JSON in $EDITOR

campaign-edit keyleth class add Druid 5 --casting wis   # gives her a 5th-level caster's slots
campaign-edit keyleth spell add Moonbeam --level 2 --concentration
campaign-edit keyleth cast moonbeam --at 3
campaign-edit grog use rage
campaign-edit grog rest short
```

`import` takes the same JSON as `edit` (and `<CHARACTER> show --json`
prints, so an exported character imports again, e.g. into another
campaign); unknown fields are refused, and names already taken are
skipped. `data/characters/` is ignored by git, so it's a place to keep
your own.

`campaign-edit <CHARACTER> help` lists every command. Edits retry
automatically if someone else saved the character in the meantime, so they
merge rather than overwrite; `edit` reports the conflict instead, since a
whole edited document can't be re-applied safely.

### Fight screen

```sh
campaign-edit fight --new "Goblin ambush"   # start a combat
campaign-edit fight --plan "goblin ambush"  # start one from a plan, monsters and all
campaign-edit fight                         # the latest one still running
campaign-edit fight goblin                  # one by name or ID prefix
export CAMPAIGN_COMBAT_SERVER=http://127.0.0.1:3001   # the default
export CAMPAIGN_MONSTER_SERVER=http://127.0.0.1:3003  # the default
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

Monsters come from the monster service (see [Monsters](#monsters)); Tab
completes their names with their CR, type and hit points, and their stat
block shows in the side panel when they act. Several of one monster are
numbered on from any already in the fight.

Every action but `use` takes any combatants as targets. Effects need
none, for things in the area like a fire on the floor; one with a duration
lasts that many rounds counted in turns of whoever put it down (the
acting combatant), and the initiative list shows the rounds left. As you type, the
line below says who does what to whom ("Vex deals ? damage to Goblin — how
much?"), the acting combatant and the targets light up in the initiative
order, and the sheet panel shows whoever acts. Tab completes the action,
spell, feature, effect (the conditions, and effects already in play) or
character; typing a name where a combatant goes, first target included,
completes it to their number. ↑/↓ pick among the suggestions, Esc clears
the line, PgUp/PgDn scroll the log, Alt-↑/↓ scroll the sheet panel, Ctrl-C
quits.

When the sheet doesn't fit its panel, it opens at the part the line needs:
spellcasting for a cast, features for a use, a monster's actions for
whatever it does. The panel's bottom edge says how much is hidden above and
below.

The panel describes what the line is about, too (see [Spells and
conditions](#spells-and-conditions)): the spell being cast, under the
caster's slots, as soon as Tab would pick it; the effect being put down or
dropped, with whom an effect in play is on, who's concentrating on it and
how long it has left. `show` keeps a description, or a combatant's sheet,
there while the fight goes on, until another line runs or Esc on an empty
line.

Casting records the cast in the combat log (from a pact slot when that's
what works) and, for concentration spells, puts the spell into play as an
effect on its targets; `use` logs a feature's use. The combat service spends
the slot or use on the sheet (see [Character
sheets](#character-sheets-in-step-with-combats)), and gives it back if the
cast is refused or undone. Changes show at once and are confirmed by the
server; if the connection drops the screen keeps working, queues what you
do and sends it once it has reconnected.

### Planning encounters

```sh
campaign-edit plan                     # the plan changed last
campaign-edit plan "Goblin ambush"     # that one, or a new one by that name
```

A screen for putting fights together ahead of time. It shows the open
plan's monsters, how hard they'd be for the party, the campaign's plans, and
the stat block of the monster you're typing (Alt-↑/↓ scroll it). Plans are kept in the campaign
(by the combat service), so every DM device sees them, and they can be used
again and again:

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

A fight from a plan holds only its monsters, numbered as `add` numbers them;
characters join in the fight screen as usual.

## Monsters

The monster service keeps full stat blocks: a **shared library**, which
every campaign sees the part of that suits its edition, and each
campaign's **own** (homebrew), which only that campaign sees and can
change.

The SRD monsters aren't part of this repository. `make srd` (that is,
`tools/fetch-srd.sh`) downloads them into `data/monsters/`: SRD
5.1 for 2014 campaigns and SRD 5.2 for 2024, 334 and 341 monsters. Wizards
of the Coast publishes the SRD only as PDFs, so the script takes the
structured version from the community-maintained
[5e-database](https://github.com/5e-bits/5e-database), pinned to one commit
and checked against known checksums, and converts it with
`tools/convert-5e-database.py`. The SRD is licensed under CC BY 4.0; the
attribution is in `data/monsters/README.md`. Compose's seed job loads
whatever is in `data/monsters/` into the shared library on every start
(`monster-service import` does it by hand), so `make srd` then `make up`.

```sh
campaign-edit monster list goblin           # name, CR, type, AC, HP, where from
campaign-edit monster show "goblin boss"    # the stat block
campaign-edit monster export troll > cave-troll.json   # a starting point
campaign-edit monster add cave-troll.json   # the campaign's own: one stat block, or a list
campaign-edit monster edit troll            # in $EDITOR; a shared one is saved as the campaign's copy
campaign-edit monster delete "cave troll"   # the campaign's own only
monster-service import my-monsters.json [--campaign <ID>]   # admin: shared library, or a campaign
```

A stat block is JSON; everything but `name` is optional (a file can hold
one, or a list). Unknown fields are refused, so typos don't vanish:

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

`initiative` overrides the Dexterity modifier; `edition` (`"2014"` or
`"2024"`) limits a shared monster to campaigns of that edition, and left
out suits both. A monster joins a fight at the most its `hit_dice` can
come to (`hit_points` if it has none), or rolled with `add ... roll`.

A converter from another source only has to write this format, like
`tools/convert-5e-database.py` does for 5e-database; put its output in
`data/monsters/` and the seed job loads it too.

| Method | Path | Body | Response |
| --- | --- | --- | --- |
| `GET` | `/monsters?search=…` | | summaries: the shared ones of the campaign's edition and its own |
| `GET` | `/monsters/{id}` | | a stat block (shared, or the campaign's own) |
| `POST` | `/monsters` | stat block | `201`, added to the campaign's own |
| `PUT` | `/monsters/{id}` | stat block | replaced (the campaign's own only; shared is `403`) |
| `DELETE` | `/monsters/{id}` | | `204` (the campaign's own only) |
| | `/spells`, `/spells/{id}` | spell | the same for spells, but the list is of whole spells |
| | `/conditions`, `/conditions/{id}` | condition | and for conditions |

## Spells and conditions

The monster service also keeps the full text of spells and conditions,
shared and per campaign exactly like monsters, which the fight screen shows
(see [Fight screen](#fight-screen)). `make srd` fetches the SRD's into
`data/spells/` and `data/conditions/` (319 and 339 spells, and the fifteen
conditions, for SRD 5.1 and 5.2), and the seed job loads them. Spells
outside the SRD can be added as the campaign's own; until then the fight
screen shows what a character's sheet says about one (its notes).

```sh
campaign-edit spell list fire               # name, level and school, C for concentration, source
campaign-edit spell show fireball           # the description
campaign-edit spell export bless > my-spell.json   # a starting point
campaign-edit spell add my-spell.json       # the campaign's own: one, or a list
campaign-edit spell delete "my spell"       # the campaign's own only
campaign-edit condition list                # the same for conditions
campaign-edit condition show grappled
monster-service import --kind spells my-spells.json [--campaign <ID>]   # admin
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

## Character API

| Method | Path | Body | Response |
| --- | --- | --- | --- |
| `GET` | `/health` | | `ok` |
| `GET` | `/characters` | | `200` list of summaries |
| `POST` | `/characters` | sheet | `201` character + `Location` header |
| `GET` | `/characters/{id}` | | `200` character |
| `PUT` | `/characters/{id}` | sheet + `If-Match` header | `200` character (full replace) |
| `DELETE` | `/characters/{id}` | | `204` |
| `POST` | `/characters/{id}/actions` | action | `200` updated character |
| `PUT` | `/characters/{id}/owners` | device keys | `200` character (DM devices only) |

Every route but `/health` needs `Authorization: Bearer <token>` and only
sees the token's campaign: another campaign's characters are `404`. Every
device sees every character, but a player's device changes only those it
plays (see [Who may do what](#who-may-do-what)).
Errors are `{"error": "..."}` with `400` (malformed request), `401` (no or
invalid token), `403` (not yours to change), `404` (not found),
`409` (out of slots or uses), `412`/`428` (see below) or `422` (invalid sheet
or action).

### Versioning

Every character has a `version` that goes up on each change (edits and
actions alike), returned in the body and as an `ETag: "<version>"` header.
`PUT` must send the version it was based on, so a stale copy can't silently
overwrite newer changes (e.g. undo a spell slot spent from another device):

```sh
curl -X PUT localhost:3000/characters/$ID -H 'If-Match: "3"' \
  -H 'content-type: application/json' -d @sheet.json
```

- Missing `If-Match` → `428 Precondition Required`.
- Version is out of date → `412 Precondition Failed`. The body is
  `{"error": ..., "current": <latest character>}`: re-apply your change to
  `current` and retry with its version.
- `If-Match: *` skips the check and overwrites unconditionally.

Actions don't need `If-Match`: they always apply to the latest version.

Every sheet field except `name` is optional and gets a sensible default:

```sh
curl -X POST localhost:3000/characters -H 'content-type: application/json' \
  -d '{"name":"Vex","race":"Half-Elf","classes":[{"name":"Ranger","level":5}],
       "ability_scores":{"dexterity":18},"skill_proficiencies":["stealth"]}'
```

## Resources and actions

Spell slots, pact slots and class features (Action Surge, Rage, Ki...) all
share one shape: `{ "max": N, "used": N }`. Features also carry a
`recharge` of `short_rest`, `long_rest` or `manual`.

How many spell slots a character has follows from their classes, under the
campaign's edition (chosen with `new-campaign --edition 2014|2024`); the
service works them out on every save, so any `max` sent is replaced, and
slots already spent stay spent. One spellcasting class uses its own table:
full casters (Bard, Cleric, Druid, Sorcerer, Wizard) by level, Paladins
and Rangers by half their level rounded up (from 1st level in 2024, 2nd in
2014), Artificers likewise from 1st, and Eldritch Knights and Arcane
Tricksters (by subclass) by a third, from 3rd level. Several share one pool
from the multiclass table: full casters add their level, Paladins and
Rangers half (rounded down in 2014, up in 2024), Artificers half rounded
up, third casters a third rounded down. Warlock levels give separate Pact
Magic. Classes the service doesn't know give no slots.

```json
"spellcasting": {
  "spells": [{ "name": "Shield", "level": 1 }, { "name": "Detect Magic", "level": 1, "ritual": true, "prepared": false }],
  "slots": { "1": { "max": 4 }, "2": { "max": 2 } },
  "pact_magic": { "slot_level": 2, "max": 2 }
},
"resources": [{ "name": "Action Surge", "max": 1, "recharge": "short_rest" }]
```

In-play changes go through `POST /characters/{id}/actions`. Each action is
applied atomically, so two clients can't both spend the last slot. Sent
with an `Idempotency-Key` header, an action is applied once per key:
sending it again returns the character unchanged, so it's safe to retry
(one that failed isn't remembered, and can be):

```json
{ "type": "cast_spell", "spell": "Shield" }
{ "type": "cast_spell", "spell": "Hex", "at_level": 2 }
{ "type": "cast_spell", "spell": "Hex", "pact": true }
{ "type": "cast_spell", "spell": "Detect Magic", "ritual": true }
{ "type": "expend_spell_slot", "level": 3 }   // also restore_spell_slot
{ "type": "expend_pact_slot" }                // also restore_pact_slot
{ "type": "use_resource", "name": "Action Surge", "amount": 1 }   // also restore_resource
{ "type": "rest", "kind": "short" }           // or "long"
```

A short rest restores `short_rest` features and pact slots. A long rest restores
everything except `manual` features, plus all spell slots and current HP.

## Testing and quality checks

```sh
make test     # every test, against a throwaway Postgres of their own
make check    # formatting, lints (warnings fail), the tests and the guard hook: what CI runs
make deny     # dependencies: licences, security advisories, yanked crates (cargo install cargo-deny)
make smoke    # end to end: the compose stack, seeded, driven by campaign-edit (needs make srd)
cargo test -p campaign-types -- --ignored   # the SRD data from make srd loads and is valid

tools/sandbox.sh up                    # a whole stack of your own to try things in
tools/sandbox.sh run list              # campaign-edit, in it
tools/sandbox.sh tmux fight fight --new Test   # a full screen, in a detached tmux session
tools/sandbox.sh down                  # stop it and delete everything in it
```

**Nothing touches a real stack.** Tests, experiments and checks never
reach the stack `make up` runs, its databases, or this machine's
`campaign-edit` config:

- *Tests.* The services' tests need Postgres, and only ever a throwaway
  one: `make test` (`tools/with-test-postgres.sh`) starts one on a port of
  its own and deletes it afterwards. There's no default: `cargo test`
  without `TEST_POSTGRES_URL` stops, and a server holding any of a real
  stack's databases is refused (`campaign_server::testing`).
- *Trying things.* `tools/sandbox.sh` runs a complete stack (its own
  Postgres container, ports 23000–23003 and 25432, data in
  `target/sandbox`) with a campaign, the example characters and the SRD,
  and always runs this checkout's `campaign-edit` with the sandbox's config
  and servers.
- *The smoke test* runs the stack as its own compose project: its own
  image, volumes and ports (13000–13003 and 15432), all deleted afterwards.
- *Claude Code.* `.claude/settings.json` installs a hook,
  `.claude/hooks/guard.py`, that blocks shell commands which would reach
  the real stack: `campaign-edit` outside the sandbox, its containers and
  ports, compose without an isolated project, `make up`/`down`/`install`,
  and the like. It reads a command as the shell does, so `campaign-edit`
  in a grep pattern or a commit message is just text, while one run
  through `bash -c`, `$( )`, tmux, ssh or a variable still counts. `make
  check` and CI run its self-test.

The Rust version is pinned in `rust-toolchain.toml` (the Dockerfile's), and
the lints in the workspace `Cargo.toml` and `clippy.toml`: no `unsafe`, no
`unwrap` outside tests, and no casts that can silently change a number.
`deny.toml` allows only permissive licences.

On GitHub, `.github/workflows/ci.yml` runs on every push to master and every
pull request: formatting and lints; the tests against a Postgres service,
the SRD ones included, with a coverage report; cargo-deny and cargo-machete
(unused dependencies); and the smoke test. Dependabot opens weekly pull
requests for newer crates, base images and actions. A `v*` tag runs
`release.yml`: the server image goes to the GitHub container registry, and
`campaign-edit` for Linux, macOS and Windows to a GitHub release. Protect
`master` so pull requests need CI to pass.

## Combat service

Combats are **event-sourced**. Clients send *commands* (intents: "deal 7
damage to the goblin"); the server *decides* what *events* (facts: "goblin
took 7, now at 0 HP") they produce, appends them to a permanent log, and
broadcasts them to every connected client. A combat's state is never stored
directly; it's the result of replaying its log. The decide/apply rules live
in `campaign-types::combat`, so clients run exactly the same code.

| Method | Path | Body | Response |
| --- | --- | --- | --- |
| `GET` | `/combats?status=…` | | the campaign's combats (status, round, last activity), most recently active first |
| `POST` | `/combats` | `{"name": ...}`, or `{"encounter": <plan id>}` | `201` snapshot |
| `GET` | `/combats/{id}` | | snapshot: round, current turn, initiative order, effects, and `seq` |
| `GET` | `/combats/{id}/log?after=N` | | the complete log (or the part after `seq` N) |
| `POST` | `/combats/{id}/commands` | command envelope | the events it produced |
| `GET` | `/combats/{id}/ws` | | WebSocket: live events, and commands in |
| `GET` | `/encounters` | | the campaign's planned encounters, by name |
| `POST` | `/encounters` | plan | `201`, the stored plan |
| `GET` | `/encounters/{id}` | | a plan |
| `PUT` | `/encounters/{id}` | plan | replaced (the last save wins) |
| `DELETE` | `/encounters/{id}` | | `204` |

Every route but `/health` needs a token, sent as `Authorization: Bearer`
(on the WebSocket handshake too), and only sees the token's campaign. The
`/encounters` routes are for DM devices only; a player's token gets `403`. A
command envelope carries a client-chosen `command_id` (events are tagged
with it, and resending it is a no-op), and the command. The log's `actor`
is always the sending device's name from its token, whatever the client
puts there:

```json
{ "command_id": "…", "actor": "Vex", "command": { "type": "damage", "combatant": "…", "amount": 5 } }
```

Commands: `add_combatant` (any time, also mid-combat), `add_monsters`,
`remove_combatant`,
`roll_initiative` (pass `roll` for physical dice, omit it for a server roll),
`roll_initiative_for_all`, `set_initiative`, `start`, `next_turn`, `damage`
(temporary HP first), `heal`, `set_temporary_hit_points`, `add_effect`,
`remove_effect`, `cast_spell`, `use_feature`, `pause`, `resume`, `end`,
`undo`.

`cast_spell` (`caster`, `spell`, optional `slot` such as `{"level":2}` or
`{"pact":3}`, `targets`) and `use_feature` (`combatant`, `feature`,
`amount`) put spells and features in the log. The slot or use itself is
spent on the character sheet through the character service; the combat
only records it. A concentration spell passes `concentration`, the id of
the effect that puts it into play on its targets, which ends the caster's
previous concentration.

Rules the server applies on its own, as extra events of the same command:
new rounds when the turn order wraps; effects with a duration expiring at
the start of their source's turn; a creature's previous concentration effect
ending when it concentrates on a new one, or when it drops to 0 HP; the turn
passing on when the current combatant is removed.

**WebSocket.** The server first sends `{"type":"snapshot","data":…}`, then
`{"type":"event","data":<logged event>}` for every change by anyone, in log
order. Commands go up as `{"type":"command","data":<envelope>}`. A refused
command gets `rejected` (to its sender only); a repeated one gets
`already_applied`. A client that falls too far behind gets a fresh snapshot.

**Undo.** `{"type":"undo"}` takes back the sending device's latest
command that isn't undone yet; repeat it to step further back, or pass
`"command": "<command_id>"` to undo a particular one. A device can only undo
its own commands (recognised by its key, not its name). Nothing is removed
from the log: the undo appends an `undone` event followed by events that put
back what the command changed (`combatant_restored`, `effect_restored`,
`progress_restored`, or `effect_ended` with reason `undone`). Changes others
made since are kept:

- hit points move back by the amount the command changed them, so undoing
  7 damage after someone else dealt 2 leaves the 2;
- initiative goes back unless someone changed it again since;
- combatants and effects it added go away, ones it removed or ended come
  back (a concentration effect only if its source hasn't started
  concentrating on something else);
- round, turn and status go back only if nothing has moved them since;
  otherwise the undo is refused ("the combat has moved on since"), as it
  is if the combatant whose hit points it would restore has left.

An ended combat accepts only the undo of `end`. An undo can't itself be
undone. HP write-back follows undos like any other change. Undoing a cast
ends the effect it put into play and gives its slot back on the sheet, and
undoing a feature's use gives the use back.

**Optimistic UIs.** `campaign_client::combat::Replica` keeps the confirmed
state plus the client's in-flight commands and shows their predicted result
immediately, replaying them on top of whatever the server confirms first and
dropping them if rejected. New combatants and effects carry client-generated
ids so the prediction matches the server exactly; only server dice rolls
and undos (which need the log) wait for the server.

**Pausing.** `pause` interrupts an active combat, e.g. at the end of a
session; until `resume` only `end` is accepted. Nothing about a combat lives
only in memory, so it can be resumed days later, on any instance, exactly
where it stopped: `GET /combats?status=paused` finds it. Other
combats, in the same campaign or others, run independently meanwhile.

**Several instances.** The database is the only authority. Each command
locks its combat's row (`SELECT … FOR UPDATE`), so commands to one combat
are applied one at a time whichever instance receives them, while different
combats never wait for each other. Before deciding, an instance catches its
cached state up from the log. Every append sends a Postgres `NOTIFY`; each
instance listens, reads the new events and pushes them to its own WebSocket
clients, so everyone sees every change in log order. Resent commands are
recognised through the `combat_commands` table. Combats nobody is connected
to are dropped from memory after ten minutes and rebuilt from the log when
next used.

<a id="character-sheets-in-step-with-combats"></a>
**Character sheets in step with combats.** A combatant added with
`Combatant::from_character` carries a `character_id`, and the service keeps
that character's sheet in step, so every frontend gets it for free:

- *Slots and uses.* Before running a `cast_spell` with a slot or a
  `use_feature`, the service spends the slot or uses on the sheet; with
  none left the command is refused (`422`), with the sheet's own reason. If
  the rules then refuse the command it gives them back, and undoing it gives
  them back before the undo returns. Each of these carries an
  `Idempotency-Key` (the command's id, or one derived from it), which the
  character service applies once, so a resent command or a retry never
  spends or gives back twice.
- *Hit points.* After every command that changes such a combatant's HP, the
  service writes its current and temporary HP back to the sheet (current
  capped at the sheet's maximum). The write-back is one-way: editing a
  sheet's HP during a combat doesn't change the combatant.

Progress is tracked per combat in `combat_hp_sync`, so if the character
service is unreachable, write-back and give-backs are retried every 30
seconds; only one instance syncs a combat at a time and it always writes
the latest HP, so sheets never go backwards. With `CHARACTER_SERVICE_URL`
empty, sheets are left alone entirely.

### Adding monsters

`add_monsters` (`monster`, `count`, `roll`) adds monsters from the
campaign's own and shared ones by id. The combat service fetches the stat
block from the monster service (as itself, like the sheets), names and
numbers them on from any already in the fight ("Goblin 3"), and gives them
their maximum hit points or, with `roll`, rolls them with its own dice.
Clients can't predict it, so it shows up once the server answers; undoing
it takes all of them out again.

`POST /combats` with `{"encounter": <plan id>}` (DM devices only) starts a
combat from a planned encounter, named after the plan unless a `name` is
given, with the plan's monsters already in it. It's all or nothing: if a
monster in the plan is gone from the campaign, there's no combat, and the
error says which. With `MONSTER_SERVICE_URL` empty, both are refused.

## Authentication

Every device (the DM's laptop, a player's phone) has its own Ed25519 key,
generated on the device and never sent anywhere: `campaign-edit` keeps it in
`~/.config/campaign-manager/device.key` (or `$CAMPAIGN_CONFIG_DIR`).
Nobody has passwords.

**Certificates.** A device belongs to a campaign when a DM device of that
campaign has signed a certificate for it: *this key, this campaign, this
role, this name, signed by that DM key*. The auth service only accepts a
certificate signed by the DM device presenting it, and keeps it on record;
`campaign-edit auth devices` checks every certificate itself. A campaign's
first DM device is set up with a one-time code printed by `auth-service
new-campaign`, and signs its own certificate. If every DM device is lost,
the server's admin can print a new code with `auth-service setup-code
<CAMPAIGN>`; whoever runs the server can always do that.

**Pairing.** Rather than copying keys around, the DM's device shows a code
like `7-tavern-dragon` that is read out and typed on the new device. The
number only picks the pairing on the auth service; the two words (from the
EFF short wordlist, 1,296 words) are a password that never leaves the two
devices. They run SPAKE2 with it (a PAKE: password-authenticated key
exchange), the auth service relaying the messages, and agree on a strong
key only if both used the same words. Nobody watching, the auth service
included, learns anything to guess the words against offline. Someone who
heard only the number gets one online guess: a wrong guess fails to
decrypt, the DM's device cancels the pairing, and only one device can ever
claim a code anyway. That makes two words (one chance in 1.7 million)
plenty.

Over the resulting channel (ChaCha20-Poly1305, a key per direction, from
HKDF-SHA256) the new device sends its key and name; the DM is asked to
approve, and the DM's device adds the device with a certificate and sends
back the certificate, the campaign, and the auth service's key. So a
paired device trusts the auth service because the DM's device vouched for
it, not merely because it answered first. Codes expire after 10 minutes.
The relay keeps messages in Postgres and clients long-poll, so the two
devices may reach different auth-service instances.

**Logging in** is challenge–response. The device asks for a challenge (a
random nonce, single-use, valid for a minute) and signs it together with the
campaign and the auth service's own key, so the signature is useless
anywhere else. In return it gets a token valid for 15 minutes: a PASETO
v4.public token, Ed25519-signed by the auth service, saying which device it
is, its campaign, role and name. The other services check tokens with the
auth service's public key alone, without calling it. Devices remember the
auth service's key from the first time they use it and refuse to log in to
any other.

**Signatures** always cover `"campaign-manager/v1/<purpose>\0"` followed
by the value's canonical binary (postcard) encoding, so a signature made
for one purpose (say, logging in) can never pass as another (a
certificate).

**Roles.** DM devices admit and revoke devices; the campaign always keeps at
least one. Players use the services. Services (registered with `auth-service
add-service`) may log in to any campaign, so the combat service can write
hit points back.

### Who may do what

A player changes only their own character. Whatever happens to anyone
else, the DM or that character's player enters on their own screen. So:

- **Characters.** A character belongs to the devices that play it: a
  player's device plays what it creates, and the DM chooses who plays the
  rest (`campaign-edit <CHARACTER> owners add <DEVICE>`, with the device's
  name or key prefix from `auth devices`; a player's phone and laptop can
  both play one). A player's device changes only the characters it plays;
  a DM's changes any. Every device sees every character.
- **Fights.** A player damages, heals and gives temporary hit points only
  to their own combatants, uses only their features, and casts spells and
  puts down effects that name only themselves. The exception is their own
  concentration: Bless on the party or Hunter's Mark on a goblin may name
  anyone, since it only marks whom it's on and ends with their
  concentration (they may end it too). They end their own turn and
  undo their own commands. Everything else (starting a combat, who's in
  it, initiative, pausing and ending) is the DM's. The combat service
  checks this, asking the character service whose characters are whose,
  and the fight screen says so before sending.
- **What players see of a fight.** The party knows each other's hit
  points, but monsters' (and anyone else the DM adds) are behind the DM's
  screen: a player's device sees only how hurt one looks (unhurt, hurt,
  bloodied at half or less, down), not its numbers, armor class or stat
  block. The combat service conceals them in everything it sends a
  player's device: snapshots, the log and live events.
- **Monsters, spells, conditions and plans** are the DM's to change; plans
  are hidden from players altogether.

| Method | Path | Body | Response |
| --- | --- | --- | --- |
| `GET` | `/public-key` | | the key tokens are signed with |
| `POST` | `/setup/lookup` | `{"code"}` | the campaign a setup code is for |
| `POST` | `/setup` | `{"code", "cert"}` (self-signed, DM) | `201` campaign |
| `POST` | `/login/challenge` | `{"campaign", "key"}` | challenge |
| `POST` | `/login` | `{"challenge", "signature"}` | `{"token", "claims"}` |
| `GET` | `/campaign` | | the token's campaign |
| `GET` | `/devices` | | every device, with its certificate |
| `POST` | `/devices` | signed certificate (DM only) | `201` device |
| `DELETE` | `/devices/{key}` | (DM only) | the revoked device |
| `POST` | `/pairings` | (DM only) | `201` `{"slot", "expires_at"}` |
| `PUT` | `/pairings/{slot}/host/{n}` | `{"data"}` (the DM device that opened it) | `201` |
| `PUT` | `/pairings/{slot}/joiner/{n}` | `{"data"}` | `201`, or `409` if already claimed |
| `GET` | `/pairings/{slot}/{host,joiner}/{n}` | | the message (waits up to 20 s; `204` if not yet) |
| `DELETE` | `/pairings/{slot}` | (the DM device that opened it) | `204` |

Revoking a device stops it logging in; a token it already has stays valid
until it expires (at most 15 minutes), and an open WebSocket stays open.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  https://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or https://opensource.org/licenses/MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.

### Third-party material

- `crates/campaign-auth/src/eff_short_wordlist_1.txt` is the EFF's short
  wordlist 1 (https://www.eff.org/dice), licensed under CC BY 3.0 US
  (attribution in the file; "yo-yo" is written "yoyo").
- The SRD monsters, spells and conditions are not included; see
  [Monsters](#monsters) for how `make srd` fetches them, and
  `data/monsters/README.md` (and its neighbours) for their
  attribution (System Reference Document 5.1 and 5.2 by Wizards of the
  Coast LLC, CC BY 4.0).
