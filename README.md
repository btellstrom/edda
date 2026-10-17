# Edda

Edda keeps a D&D 5e campaign at the table: character sheets, monsters,
spells and live combats, shared between the DM's and the players'
devices. The DM runs the servers, everyone uses `edda`, a command line
with full-screen views for fights and for planning them.

- **Character sheets** with spell slots, class features and rests worked
  out under the 2014 or 2024 rules, edited from any device without
  overwriting each other.
- **A fight screen** for initiative, damage, spells, concentration and
  conditions, live on every device, and working on through a dropped
  connection. Hit points and spent slots go back to the sheets.
- **Encounter planning** with difficulty for the party, under either
  edition's rules.
- **Monsters, spells and conditions** from the SRD, plus each campaign's
  own homebrew.
- **What's behind the DM's screen stays there.** Players see how hurt a
  monster looks, not its numbers, and change only their own characters.
- **No passwords.** Each device has its own key; the DM admits a player's
  device by reading out a code like `7-tavern-dragon`.

## Getting started

This sets up the servers on the DM's computer and the DM's own device.
[Hosting](docs/hosting.md) covers letting players' devices in.

### 1. What you need

- [Podman](https://podman.io) (with `podman compose`) or Docker with
  Compose, for the servers. With Podman, enable its API socket once:
  `systemctl --user enable --now podman.socket`.
- [Rust](https://rustup.rs), for `edda`. With rustup, the right version
  is picked up from `rust-toolchain.toml`.
- `make`, and for the SRD: `curl`, `sha256sum` and `python3`.

The Makefile uses Podman; add `CONTAINER=docker` to every `make` command to
use Docker.

### 2. Start the servers

```sh
git clone https://github.com/btellstrom/edda.git
cd edda
make srd    # download the SRD's monsters, spells and conditions (optional)
make up     # build the servers' image and start them
```

The servers run in the background and keep their data in volumes, across
restarts too; `make down` stops them, `make logs` follows their logs.

### 3. Create a campaign

```sh
make campaign NAME="Vox Machina" EDITION=2024    # or EDITION=2014
```

This prints a one-time setup code for the campaign's first DM device.

### 4. Set up your device

```sh
make install                                     # edda into ~/.cargo/bin
edda auth setup <SETUP CODE> --name "DM laptop"
echo 'source <(COMPLETE=bash edda)' >> ~/.bashrc   # tab completion (zsh and fish: see the guide)
```

### 5. Play

```sh
edda new Grog --race Goliath --player Travis
edda grog class add Barbarian 3
edda fight --new "Goblin ambush"
```

In the fight screen, `add grog`, `add 3 goblin warrior`, `roll`, `start`, then
`damage 2 7`, `next` and so on; Tab completes everything.

To bring in players, their devices need to reach the servers: put them
behind HTTPS ([Hosting](docs/hosting.md#letting-players-in-https)), then
pair each device with `edda auth invite`.

## Documentation

| Guide | For |
| --- | --- |
| [Using edda](docs/using-edda.md) | Everyone at the table: devices, campaigns, characters, the fight screen and planning encounters. |
| [Monsters, spells and conditions](docs/monsters-and-spells.md) | DMs: the SRD, and writing your own. |
| [Hosting](docs/hosting.md) | Whoever runs the servers: HTTPS for players, limits, backups, running without containers. |
| [Security](docs/security.md) | How devices, pairing and logging in work, and who may do what. |
| [API](docs/api.md) | Building another frontend: the services' HTTP and WebSocket API. |
| [Development](docs/development.md) | Contributing: the code's layout, tests, the sandbox and CI. |

## How it fits together

Four small Rust services, each with its own Postgres database: **auth**
(campaigns, devices, logging in), **characters**, **combat** (live,
event-sourced fights over WebSockets) and **monsters** (monsters, spells
and conditions). They are built into one container image and run with
compose. `edda` talks to them over HTTP; the shared crates let other
frontends do the same.

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

- `crates/edda-auth/src/eff_short_wordlist_1.txt` is the EFF's short
  wordlist 1 (https://www.eff.org/dice), licensed under CC BY 3.0 US
  (attribution in the file; "yo-yo" is written "yoyo").
- The SRD monsters, spells and conditions are not included; `make srd`
  fetches them (see [Monsters, spells and
  conditions](docs/monsters-and-spells.md#the-srd)), and
  `data/monsters/README.md` (and its neighbours) give their attribution
  (System Reference Document 5.1 and 5.2 by Wizards of the Coast LLC,
  CC BY 4.0).
