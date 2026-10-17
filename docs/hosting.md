# Hosting

For whoever runs the servers. The [README](../README.md#getting-started)
gets them running on one machine; this covers the rest.

## What runs

`compose.yaml` runs Postgres and four services, all from one image built
by the `Dockerfile`:

| Service | Port | Keeps |
| --- | --- | --- |
| auth | 3002 | campaigns, their devices, logging in |
| characters | 3000 | character sheets |
| combat | 3001 | combats and planned encounters |
| monsters | 3003 | monsters, spells and conditions |
| postgres | 5432 | a database for each |

and two jobs that run at every start: one registers the combat service's
key, so it can write hit points back to character sheets, and the other
loads `data/` into the monster service's shared library.

The ports are published on localhost only. Move any of them with
`AUTH_PORT`, `CHARACTERS_PORT`, `COMBAT_PORT`, `MONSTERS_PORT` or
`POSTGRES_PORT`, e.g. if another Postgres already has 5432:
`POSTGRES_PORT=5433 make up`.

```sh
make up                     # build and start (or update) the servers
make down                   # stop them; the data stays in their volumes
make logs                   # follow their logs
make campaign NAME="Vox Machina" EDITION=2024    # a new campaign's setup code
make delete-campaign ID=<id>   # a campaign and everything in it (asks first; YES=1 doesn't)
make                        # every target
```

`make` uses Podman; `make CONTAINER=docker ...` uses Docker. Without make,
it's plain compose: `podman compose up -d --build`, `podman compose exec
auth auth-service new-campaign "Vox Machina" --edition 2024`, and so on.

**Updating** is `git pull` and `make up`: the services update their
databases themselves on start.

**Is it up?** Every service answers `GET /health` with `ok` while it runs,
and `GET /ready` with `ready` while it can also reach its database (`503`
while it can't): point a load balancer's or Kubernetes' checks at
`/ready`. A service asked to stop (Ctrl-C, or SIGTERM as compose and
Kubernetes send) finishes the requests in progress first.

**A campaign's id** is shown by `edda auth status` on any of its devices.
Deleting a campaign removes it from every service: its devices (which can
no longer log in), characters, combats, plans and its own monsters.

**If a campaign loses every DM device,** `podman compose exec auth
auth-service setup-code <CAMPAIGN ID>` prints a new setup code for it.

**Logs** go to the containers' output (`make logs`), a line a request
with its method, path, status and time, and the campaign it was for.
`LOG_FORMAT=json` makes each line a JSON object, for a log collector;
`RUST_LOG` sets how much (`RUST_LOG=debug`). They never hold what was
sent or asked, only whose it was. With `OTEL_EXPORTER_OTLP_ENDPOINT` set
(say `http://collector:4318`), each request is also sent as a trace to
that OpenTelemetry collector, over HTTP. Every response carries an
`x-request-id`, also in the logs, so a user's report can be found there.

**Metrics** for Prometheus: with `METRICS_ADDR` set (say
`0.0.0.0:9100`), a service serves `/metrics` there, an address of its own
that the proxy never serves; without it, nothing is measured.

| Metric | |
| --- | --- |
| `edda_http_requests_total` | requests, by `route` (its template, `/characters/{id}`), `method` and `status` |
| `edda_http_request_duration_seconds` | how long they took, by `route` and `method` |
| `edda_limits_refused_total` | requests a [limit](#limits) refused, by `limit` |
| `edda_db_connections` | the service's database connections, `busy` or `idle` |
| `edda_combat_watchers` | open WebSockets to combats, by `account` (from the hosting's proxy) |
| `edda_combat_hp_sync_behind` | combats whose hit points haven't reached the sheets yet |

Each is labelled with its `service`.

## The admin API

What `make campaign` and `make delete-campaign` do, over HTTP, for a
program that runs the servers (a hosting's control plane, say). With
`ADMIN_ADDR` (an address only that program can reach, never the proxy)
and `ADMIN_TOKEN` (32 or more printable ASCII characters) both set, a
service serves it there; each request must bear the token
(`Authorization: Bearer <token>`).

| Service | Method | Path | Answers |
| --- | --- | --- | --- |
| auth | `POST` | `/admin/campaigns` `{"name", "edition"}` | `201` `{"campaign", "setup_code"}` |
| auth | `POST` | `/admin/campaigns/{id}/setup-code` | `{"code"}`: a new code for a DM device |
| every one | `DELETE` | `/admin/campaigns/{id}` | `{"deleted": n}`: its part of the campaign; safe to repeat |

In Rust, `edda_client::admin::AdminClient` calls it.

## Letting players in: HTTPS

To let players' devices in, put the stack behind HTTPS rather than
opening the services themselves: the `https` profile adds a proxy
(Caddy, configured in `proxy/Caddyfile`) that serves all four services,
WebSockets included, on one address.

```sh
make up HTTPS=1 EDDA_HOST=dnd.example.com    # a domain pointing at this machine
make up HTTPS=1 EDDA_HOST=192.168.1.20       # or its address on your network
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
  run `update-ca-certificates`), or for edda alone set
  `SSL_CERT_FILE=root.crt`.

Then point edda at it ([Using edda](using-edda.md#where-the-servers-are)),
and invite players with the address they should use: `edda auth invite
--auth-server https://dnd.example.com:8443`. A device logs in through the
address it paired with.

**Browsers.** The services answer web pages from other sites only if
`CORS_ORIGINS` lists them (comma-separated origins such as
`https://dnd.example.com`, or `*` while developing). Unset, none may:
edda doesn't need it, and neither does a page served through the proxy.

## Limits

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
up`, or `LIMITS=off` in a `.env` file next to `compose.yaml`). A client
over its rate gets `429` with `Retry-After`; a full campaign gets `403`
saying which limit it reached.

Requests are counted per service instance and client address. Behind the
proxy, `TRUST_PROXY=1` (set in compose) takes the address from the
`X-Forwarded-For` the proxy adds and counts only requests that came
through it, so the services' calls to each other aren't limited; set it
only where nothing else can reach the services.

## Backups

```sh
make backup                           # into backups/<date and time>; the servers keep running
make restore FROM=backups/<date>      # replaces all data with the backup's
```

A backup holds the four services' databases (`pg_dump`) and the combat
service's key. Restoring stops the four services, puts it all back and
starts them again. Copy `backups/` somewhere else: a backup on the same
disk is lost with it.

## Running without containers

Every service needs Postgres. Start one with `podman compose up -d
postgres`, or without compose:

```sh
podman run -d --name edda-postgres -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:5432:5432 -v edda-postgres:/var/lib/postgresql/data \
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
# DATABASE_URL defaults to postgres://postgres:postgres@localhost:5432/combats,
#   BIND_ADDR to 127.0.0.1:3001,
# CHARACTER_SERVICE_URL to http://127.0.0.1:3000 (empty leaves character sheets alone),
# MONSTER_SERVICE_URL to http://127.0.0.1:3003 (empty: no adding monsters by id)
cargo run -p monster-service -- import data/monsters/*.json   # the SRD, once
cargo run -p monster-service -- import --kind spells data/spells/*.json
cargo run -p monster-service -- import --kind conditions data/conditions/*.json
cargo run -p monster-service
# DATABASE_URL defaults to postgres://postgres:postgres@localhost:5432/monsters,
#   BIND_ADDR to 127.0.0.1:3003
```

Each creates its database on first start. The combat service also
creates its own key then (`SERVICE_KEY_FILE`, default
`combat-service.key`) and logs the command that lets it write hit points
back: `auth-service add-service combat-service <KEY>` (`combat-service
public-key` prints the key).

**Behind PgBouncer.** With many service instances, a pooler such as
PgBouncer keeps Postgres from running out of connections. In its
transaction mode, set `DATABASE_URL` to the pooler and
`DATABASE_DIRECT_URL` to Postgres itself: creating the database, applying
migrations and the combat service's `LISTEN` need a session of their own,
so they go there. PgBouncer 1.21 or later, with `max_prepared_statements`
set, handles the services' prepared statements.

**Several combat services** can run against the same database, with
different `BIND_ADDR`s behind any load balancer, and clients may connect
to whichever they like; see [the API](api.md#several-instances).

A campaign deleted by hand is `delete-campaign <ID>` with each of
`auth-service`, `character-service`, `combat-service` and
`monster-service`; each deletes its own part and can be re-run safely.
