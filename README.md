# Campaign Manager

A collection of small Rust services for running a D&D campaign, with state held
server-side and any number of frontends talking to it over HTTP/JSON.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/campaign-types` | Shared domain models, rules helpers (modifiers, proficiency) and validation. No server deps, so frontends (including WASM) can depend on it. |
| `crates/campaign-auth` | Device keys, signed device certificates and tokens. Shared by the services and clients. |
| `crates/auth-service` | Campaigns, the devices admitted to them, and login (axum + Postgres). |
| `crates/character-service` | Character sheet CRUD API (axum + SQLite). |
| `crates/combat-service` | Live, event-sourced combat encounters (axum + Postgres + WebSocket). Runs as any number of instances. |
| `crates/campaign-client` | Typed async client for the services, plus `Replica` for optimistic UIs. Shared by all frontends. |
| `crates/campaign-edit` | Git-style command-line frontend for editing character sheets. |

## Running

The combat service needs Postgres. Start one with `docker compose up -d`
(see `compose.yaml`), or without a compose provider:

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
cargo run -p auth-service -- new-campaign "Vox Machina"   # prints the DM's setup code

cargo run -p character-service
# DATABASE_URL defaults to sqlite://characters.db, BIND_ADDR to 127.0.0.1:3000
cargo run -p combat-service
# DATABASE_URL defaults to postgres://postgres:postgres@localhost:5432/combats
#   (created on first start), BIND_ADDR to 127.0.0.1:3001,
# CHARACTER_SERVICE_URL to http://127.0.0.1:3000 (empty turns HP write-back off)
```

On first start the combat service creates its own key (`SERVICE_KEY_FILE`,
default `combat-service.key`) and logs the command that lets it write hit
points back: `auth-service add-service combat-service <KEY>`.

Run more combat-service instances against the same database (with
different `BIND_ADDR`s, behind any load balancer) and clients may connect to
whichever they like.

## Command-line editor

```sh
cargo install --path crates/campaign-edit   # or: cargo run -p campaign-edit -- <args>
export CAMPAIGN_SERVER=http://127.0.0.1:3000  # the default; or pass --server
```

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

Top-level commands manage the roster; anything else is a character (name,
unique name prefix, or ID prefix) followed by a command:

```sh
campaign-edit list
campaign-edit new Grog --race Goliath --player Travis

campaign-edit grog                       # show the sheet
campaign-edit grog set max-hp 50
campaign-edit grog set hp -7             # +N heals, -N damages
campaign-edit grog class add Barbarian 3 --subclass "Path of the Berserker"
campaign-edit grog skill add athletics intimidation
campaign-edit grog resource add Rage 3 --recharge long
campaign-edit grog item add Javelin -n 4
campaign-edit grog edit                  # whole sheet as JSON in $EDITOR

campaign-edit keyleth class add Druid 5 --casting wis
campaign-edit keyleth slots set 1 4
campaign-edit keyleth spell add Moonbeam --level 2 --concentration
campaign-edit keyleth cast moonbeam --at 3
campaign-edit grog use rage
campaign-edit grog rest short
```

`campaign-edit <CHARACTER> help` lists every command. Edits retry
automatically if someone else saved the character in the meantime, so they
merge rather than overwrite; `edit` reports the conflict instead, since a
whole edited document can't be re-applied safely.

### Fight screen

```sh
campaign-edit fight --new "Goblin ambush"   # start a combat
campaign-edit fight                         # the latest one still running
campaign-edit fight goblin                  # one by name or ID prefix
export CAMPAIGN_COMBAT_SERVER=http://127.0.0.1:3001   # the default
```

A full-screen view of a combat: the initiative order (with HP, AC and the
effects on each combatant), the sheet of whoever is acting, the log, and a
command line. Everyone on the screen has a number, and a line reads
`[WHO] ACTION [ARGS]`, where `WHO` is a number and defaults to whoever has
the turn:

```
add vex                     Vex joins the combat (from their sheet)
add Goblin 7                Goblin (7 HP) joins the combat
roll · start · next         initiative for everyone without; begin; end the turn
damage 3 7                  Vex deals 7 damage to Goblin
damage 2 3 8                Vex deals 8 damage to Grog and Goblin
2 heal 1 5                  Grog heals Vex for 5
cast hunter's mark 3        Vex casts Hunter's Mark on Goblin (concentration)
cast cure wounds 2 at 2     spends Vex's 2nd-level slot
effect Prone 3 for 1        Vex puts Prone on Goblin for 1 round
use favored foe             Vex uses Favored Foe (only ever on oneself)
init 3 17 · init 3          set Goblin's initiative, or roll it
drop hunter's mark · remove 3 · undo · pause · resume · end · quit
```

Every action but `use` takes any combatants as targets. As you type, the
line below says who does what to whom ("Vex deals ? damage to Goblin — how
much?"), the acting combatant and the targets light up in the initiative
order, and the sheet panel shows whoever acts. Tab completes the action,
spell, feature, effect or character; typing a name where a combatant goes
completes it to their number. ↑/↓ pick among the suggestions, Esc clears
the line, PgUp/PgDn scroll the log, Ctrl-C quits.

Casting spends the caster's slot on their sheet (a pact slot when that's
what works), records the cast in the combat log and, for concentration
spells, puts the spell into play as an effect on its targets; `use` spends
a feature's use and logs it. If the server refuses one of these, or you
`undo` it, the screen gives the slot or use back. Changes show at once and are confirmed by the
server; if the connection drops the screen keeps working, queues what you
do and sends it once it has reconnected.

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

Every route but `/health` needs `Authorization: Bearer <token>` and only
sees the token's campaign: another campaign's characters are `404`.
Errors are `{"error": "..."}` with `400` (malformed request), `401` (no or
invalid token), `404` (not found),
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

```json
"spellcasting": {
  "spells": [{ "name": "Shield", "level": 1 }, { "name": "Detect Magic", "level": 1, "ritual": true, "prepared": false }],
  "slots": { "1": { "max": 4 }, "2": { "max": 2 } },
  "pact_magic": { "slot_level": 2, "max": 2 }
},
"resources": [{ "name": "Action Surge", "max": 1, "recharge": "short_rest" }]
```

In-play changes go through `POST /characters/{id}/actions`. Each action is
applied atomically, so two clients can't both spend the last slot:

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

## Testing

```sh
cargo test --workspace
```

The auth and combat tests need the Postgres above; they use (and create) the
databases `auth_test` and `combat_test` on it. Point `TEST_POSTGRES_URL` at
another server if needed.

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
| `POST` | `/combats` | `{"name": ...}` | `201` snapshot |
| `GET` | `/combats/{id}` | | snapshot: round, current turn, initiative order, effects, and `seq` |
| `GET` | `/combats/{id}/log?after=N` | | the complete log (or the part after `seq` N) |
| `POST` | `/combats/{id}/commands` | command envelope | the events it produced |
| `GET` | `/combats/{id}/ws` | | WebSocket: live events, and commands in |

Every route but `/health` needs a token, sent as `Authorization: Bearer`
(on the WebSocket handshake too), and only sees the token's campaign. A
command envelope carries a client-chosen `command_id` (events are tagged
with it, and resending it is a no-op), and the command. The log's `actor`
is always the sending device's name from its token, whatever the client
puts there:

```json
{ "command_id": "…", "actor": "Vex", "command": { "type": "damage", "combatant": "…", "amount": 5 } }
```

Commands: `add_combatant` (any time, also mid-combat), `remove_combatant`,
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
ends the effect it put into play, but the slot (like a feature's use) lives
on the character sheet: whoever undoes gives it back, as the fight screen
does.

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

**Hit points on character sheets.** A combatant added with
`Combatant::from_character` carries a `character_id`. After every command
that changes such a combatant's HP, the service writes its current and
temporary HP back to the character sheet (current capped at the sheet's
maximum). Progress is tracked per combat in `combat_hp_sync`, so if the
character service is unreachable the write-back is retried every 30
seconds; only one instance syncs a combat at a time and it always writes
the latest values, so sheets never go backwards. The write-back is one-way:
editing a sheet's HP during a combat doesn't change the combatant.

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
hit points back. Beyond that, every member of a campaign can do everything
in it for now.

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
