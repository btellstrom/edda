# Development

## Layout

| Crate | Purpose |
| --- | --- |
| `crates/edda-types` | Shared domain models, rules helpers (modifiers, proficiency, spell slots, the combat rules) and validation. No server dependencies, so frontends (including WASM) can use it. |
| `crates/edda-auth` | Device keys, signed device certificates, pairing and tokens. Shared by the services and clients. |
| `crates/edda-server` | What every service shares: logging, opening its database, the auth service's key, serving (with the CORS setting and the limits) and error responses. |
| `crates/auth-service` | Campaigns, the devices admitted to them, and login (axum + Postgres). |
| `crates/character-service` | Character sheets (axum + Postgres). |
| `crates/combat-service` | Live, event-sourced combats and planned encounters (axum + Postgres + WebSocket). Runs as any number of instances. |
| `crates/monster-service` | Monster stat blocks and the text of spells and conditions: a shared library (the SRD) and each campaign's own (axum + Postgres). |
| `crates/edda-client` | Typed async client for the services, plus `Replica` for optimistic UIs. Shared by all frontends. |
| `crates/edda` | The command line, with the fight and planning screens (ratatui). |

Also: `tools/` (scripts, below), `proxy/` (the HTTPS proxy's Caddyfile),
`data/` (where `make srd` puts the SRD), and `Dockerfile` and
`compose.yaml` for the servers.

## Building and testing

```sh
make server   # the servers' container image
make client   # edda: target/release/edda
make test     # every test, against a throwaway Postgres of their own
make check    # formatting, lints (warnings fail), the tests and the guard hook: what CI runs
make deny     # dependencies: licences, security advisories, yanked crates (cargo install cargo-deny)
make smoke    # end to end: the compose stack, seeded, driven by edda (needs make srd)
make coverage # what the tests cover, failing below the floor
make doc      # the Rust docs, in a browser
make openapi  # docs/openapi.json anew, after changing a route
cargo test -p edda-types -- --ignored   # the SRD data from make srd loads and is valid
```

The services' tests need Postgres, and only ever a throwaway one: `make
test` (`tools/with-test-postgres.sh`) starts one on a port of its own and
deletes it afterwards. There's no default: `cargo test` without
`TEST_POSTGRES_URL` stops, and a server holding any of a real stack's
databases is refused (`edda_server::testing`).

`make kind` (`tools/kind-test.sh`) does the same for the Helm chart: a
kind cluster of its own, with Postgres, the chart with its admin API and
a shared token key, a campaign made through the admin API and played
through edda, all deleted afterwards. It needs kind, kubectl and helm,
and leaves `~/.kube` alone.

The smoke test runs the stack as its own compose project (`edda-smoke`):
its own image, volumes and ports (13000–13003 and 15432), all deleted
afterwards. It goes through a campaign end to end with edda, HTTPS
included, and backs up, loses a character and restores it.

## The sandbox

To try a change by hand, without touching a stack of your own:

```sh
tools/sandbox.sh up                    # a whole stack of your own to try things in
tools/sandbox.sh run list              # edda, in it
tools/sandbox.sh tmux fight fight --new Test   # a full screen, in a detached tmux session
tools/sandbox.sh https                 # the HTTPS proxy in front of it (then SANDBOX_HTTPS=1)
tools/sandbox.sh down                  # stop it and delete everything in it
```

It runs a complete stack (its own Postgres container, ports 23000–23003
and 25432, data in `target/sandbox`) with a campaign, the example
characters and the SRD, without limits, and always runs this checkout's
`edda` with the sandbox's config and servers.

## Working with Claude Code

`.claude/settings.json` installs a hook, `.claude/hooks/guard.py`, that
keeps an AI assistant's commands away from your own stack: it blocks
`edda` outside the sandbox, the stack's containers, volumes and ports,
compose without an isolated project, `make up`/`down`/`install`, and the
like. It reads a command as the shell does, so `edda` in a grep pattern or
a commit message is just text, while one run through `bash -c`, `$( )`,
tmux, ssh or a variable still counts. `make check` and CI run its
self-test.

## Documentation

The guides are in `docs/`. Two references are made from the code:

- **The Rust docs:** `make doc` builds and opens them. Every crate, type
  and function says what it is for; fields and variants do where their
  name doesn't.
- **The API description:** `docs/openapi.json`, from the routes'
  `#[utoipa::path]` attributes and the types' `ToSchema` (behind the
  `openapi` feature of `edda-types` and `edda-auth`). A test compares it
  with the code; after changing a route, `make openapi` writes it anew.

On every push to master, `.github/workflows/docs.yml` publishes both to
GitHub Pages, with a readable rendering of the API, once Pages is turned
on (Settings → Pages → Source: GitHub Actions).

## Code quality

The Rust version is pinned in `rust-toolchain.toml` (and the
Dockerfile's), and the lints in the workspace `Cargo.toml` and
`clippy.toml`: Clippy's pedantic lints but for three that only add noise,
no `unsafe`, no `unwrap` outside tests, and no casts that can silently
change a number. Where a lint is wrong about one place, an
`#[expect(..., reason = "...")]` says why.

Coverage may only go up: CI fails when the tests cover less than
`COVERAGE_FLOOR` (in the Makefile) of the lines, and `make coverage`
checks the same locally. Raise the floor as coverage rises. `deny.toml` allows only permissive
licences. Release builds use fat LTO and strip symbols, which makes them
about 40% smaller.

## CI and releases

`.github/workflows/ci.yml` runs on every push to master and every pull
request: formatting, lints and the docs; the tests against a Postgres service, the
SRD ones included, with a coverage report and floor; cargo-deny and cargo-machete
(unused dependencies); the smoke test; and the Helm chart, linted,
validated by kubeconform and installed in kind. Dependabot opens weekly pull
requests for newer crates, base images and actions.

A `v*` tag runs `release.yml`: the server image goes to the GitHub
container registry, and `edda` for Linux, macOS and Windows to a GitHub
release.
