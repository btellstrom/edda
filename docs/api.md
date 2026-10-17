# API

Everything edda does goes through the services' HTTP API, so other
frontends can do the same. In Rust, `edda-client` wraps it (typed
clients, and `Replica` for optimistic UIs), and `edda-types` holds the
models and the combat rules, so a client runs exactly the server's code.

[`openapi.json`](openapi.json) describes every route and every type, as
OpenAPI 3.1, to read or to generate a client from in any language. It is
made from the services' code, and a test fails when it falls behind
(`make openapi` writes it anew). This page explains what the description
can't: how versions, combats and undo work.

## Conventions

- **JSON** in and out. Unknown fields in a sheet or stat block are
  refused.
- **Tokens.** Every route but `/health` (and the auth service's sign-in
  routes) needs `Authorization: Bearer <token>`, from logging in (see
  [Security](security.md#logging-in)), and only sees the token's
  campaign: another campaign's things are `404`.
- **Errors** are `{"error": "..."}` with `400` (malformed request), `401`
  (no or invalid token), `403` (not yours to change, or a
  [limit](hosting.md#limits) reached), `404` (not found), `409`
  (conflict, such as no slots left), `412`/`428` (versions, below), `422`
  (invalid) or `429` (too many requests; see `Retry-After`).
- **Who may do what** is checked by the services; see [Using
  edda](using-edda.md#who-may-do-what).

| Service | Default address |
| --- | --- |
| Characters | `http://127.0.0.1:3000` |
| Combat | `http://127.0.0.1:3001` |
| Auth | `http://127.0.0.1:3002` |
| Monsters | `http://127.0.0.1:3003` |

Behind the HTTPS proxy, all four share one address.

## Characters

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

Every sheet field except `name` is optional and gets a sensible default:

```sh
curl -X POST localhost:3000/characters -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' \
  -d '{"name":"Vex","race":"Half-Elf","classes":[{"name":"Ranger","level":5}],
       "ability_scores":{"dexterity":18},"skill_proficiencies":["stealth"]}'
```

### Versions

Every character has a `version` that goes up on each change (edits and
actions alike), returned in the body and as an `ETag: "<version>"` header.
`PUT` must send the version it was based on, so a stale copy can't silently
overwrite newer changes (e.g. undo a spell slot spent from another device):

```sh
curl -X PUT localhost:3000/characters/$ID -H 'If-Match: "3"' ...
```

- Missing `If-Match` → `428 Precondition Required`.
- Version out of date → `412 Precondition Failed`. The body is
  `{"error": ..., "current": <latest character>}`: re-apply your change to
  `current` and retry with its version.
- `If-Match: *` skips the check and overwrites unconditionally.

Actions don't need `If-Match`: they always apply to the latest version.

### Resources and actions

Spell slots, pact slots and class features (Action Surge, Rage, Ki...) all
share one shape: `{ "max": N, "used": N }`. Features also carry a
`recharge` of `short_rest`, `long_rest` or `manual`.

How many spell slots a character has follows from their classes, under the
campaign's edition; the service works them out on every save, so any
`max` sent is replaced, and slots already spent stay spent. One
spellcasting class uses its own table: full casters (Bard, Cleric, Druid,
Sorcerer, Wizard) by level, Paladins and Rangers by half their level
rounded up (from 1st level in 2024, 2nd in 2014), Artificers likewise from
1st, and Eldritch Knights and Arcane Tricksters (by subclass) by a third,
from 3rd level. Several share one pool from the multiclass table: full
casters add their level, Paladins and Rangers half (rounded down in 2014,
up in 2024), Artificers half rounded up, third casters a third rounded
down. Warlock levels give separate Pact Magic. Classes the service doesn't
know give no slots.

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

## Combats

Combats are **event-sourced**. Clients send *commands* (intents: "deal 7
damage to the goblin"); the server *decides* what *events* (facts: "goblin
took 7, now at 0 HP") they produce, appends them to a permanent log, and
broadcasts them to every connected client. A combat's state is never stored
directly; it's the result of replaying its log. The decide/apply rules live
in `edda-types::combat`, so clients run exactly the same code.

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

The `/encounters` routes are for DM devices only. A player's device gets
monsters' numbers concealed in everything it's sent (snapshots, the log
and live events): only how hurt one looks.

A command envelope carries a client-chosen `command_id` (events are tagged
with it, and resending it is a no-op), and the command. The log's `actor`
is always the sending device's name from its token, whatever the client
puts there:

```json
{ "command_id": "…", "actor": "Vex", "command": { "type": "damage", "combatant": "…", "amount": 5 } }
```

Commands: `add_combatant` (any time, also mid-combat), `add_monsters`,
`remove_combatant`, `roll_initiative` (pass `roll` for physical dice, omit
it for a server roll), `roll_initiative_for_all`, `set_initiative`,
`start`, `next_turn`, `damage` (temporary HP first), `heal`,
`set_temporary_hit_points`, `add_effect`, `remove_effect`, `cast_spell`,
`use_feature`, `pause`, `resume`, `end`, `undo`.

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

**WebSocket.** The token goes on the handshake as on any request. The
server first sends `{"type":"snapshot","data":…}`, then
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
undone. Undoing a cast ends the effect it put into play and gives its slot
back on the sheet, and undoing a feature's use gives the use back.

**Optimistic UIs.** `edda_client::combat::Replica` keeps the confirmed
state plus the client's in-flight commands and shows their predicted result
immediately, replaying them on top of whatever the server confirms first and
dropping them if rejected. New combatants and effects carry client-generated
ids so the prediction matches the server exactly; only server dice rolls
and undos (which need the log) wait for the server.

**Pausing.** `pause` interrupts an active combat, e.g. at the end of a
session; until `resume` only `end` is accepted. Nothing about a combat lives
only in memory, so it can be resumed days later, on any instance, exactly
where it stopped: `GET /combats?status=paused` finds it.

### Several instances

The database is the only authority. Each command locks its combat's row
(`SELECT … FOR UPDATE`), so commands to one combat are applied one at a
time whichever instance receives them, while different combats never wait
for each other. Before deciding, an instance catches its cached state up
from the log. Every append sends a Postgres `NOTIFY`; each instance
listens, reads the new events and pushes them to its own WebSocket
clients, so everyone sees every change in log order. Resent commands are
recognised through the `combat_commands` table. Combats nobody is
connected to are dropped from memory after ten minutes and rebuilt from
the log when next used.

### Character sheets in step with combats

A combatant added with `Combatant::from_character` carries a
`character_id`, and the service keeps that character's sheet in step, so
every frontend gets it for free:

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
  capped at the sheet's maximum), undos included. The write-back is
  one-way: editing a sheet's HP during a combat doesn't change the
  combatant.

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

## Monsters, spells and conditions

| Method | Path | Body | Response |
| --- | --- | --- | --- |
| `GET` | `/monsters?search=…` | | summaries: the shared ones of the campaign's edition and its own |
| `GET` | `/monsters/{id}` | | a stat block (shared, or the campaign's own) |
| `POST` | `/monsters` | stat block | `201`, added to the campaign's own |
| `PUT` | `/monsters/{id}` | stat block | replaced (the campaign's own only; shared is `403`) |
| `DELETE` | `/monsters/{id}` | | `204` (the campaign's own only) |
| | `/spells`, `/spells/{id}` | spell | the same for spells, but the list is of whole spells |
| | `/conditions`, `/conditions/{id}` | condition | and for conditions |

Changes are for DM devices only. The formats are in [Monsters, spells and
conditions](monsters-and-spells.md).

## Auth

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

How the signatures, certificates and pairing messages are made is in
[Security](security.md).
