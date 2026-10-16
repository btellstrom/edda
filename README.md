# Campaign Manager

A collection of small Rust services for running a D&D campaign, with state held
server-side and any number of frontends talking to it over HTTP/JSON.

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/campaign-types` | Shared domain models, rules helpers (modifiers, proficiency) and validation. No server deps, so frontends (including WASM) can depend on it. |
| `crates/character-service` | Character sheet CRUD API (axum + SQLite). |

## Running

```sh
cargo run -p character-service
# DATABASE_URL defaults to sqlite://characters.db, BIND_ADDR to 127.0.0.1:3000
```

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

Errors are `{"error": "..."}` with `400` (malformed request), `404` (not found),
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
