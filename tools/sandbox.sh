#!/usr/bin/env bash
# A complete, isolated stack for trying things out by hand: its own
# Postgres container, the four services built from this checkout, a
# campaign with a DM device, and the example characters and SRD data, all
# on ports of its own and kept in target/sandbox. Nothing in it can reach a
# real stack, its databases or this machine's campaign-edit config.
#
#   tools/sandbox.sh up                 start it (2024 rules; SANDBOX_EDITION=2014)
#   tools/sandbox.sh run <args>         campaign-edit <args> in it
#   tools/sandbox.sh tmux <name> <args> campaign-edit <args> in a detached tmux
#                                       session <name>, for the full screens
#   tools/sandbox.sh player <name>      a player's device, joined to the campaign
#   tools/sandbox.sh https              the HTTPS proxy (proxy/Caddyfile) in front
#   tools/sandbox.sh status             what's running
#   tools/sandbox.sh down               stop it and delete everything in it
#
# `run` and `tmux` always use this checkout's target/debug/campaign-edit,
# with the sandbox's config and servers, and refuse to run without a
# sandbox up. They're the DM's device, or with SANDBOX_AS=<name> a player's,
# and with SANDBOX_HTTPS=1 they go through the proxy, trusting its
# certificate authority alone.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/target/sandbox"
bin="$root/target/debug"
container=${CONTAINER:-podman}
pg=campaign-manager-sandbox-pg
proxy=campaign-manager-sandbox-proxy
pg_port=25432
characters_port=23000 combat_port=23001 auth_port=23002 monsters_port=23003
https_port=28443 http_port=28080

die() { echo "sandbox: $*" >&2; exit 1; }

# The config of the device to run as: the DM's, or SANDBOX_AS's.
config() {
    if [ -n "${SANDBOX_AS:-}" ]; then
        [ -d "$dir/players/$SANDBOX_AS" ] || die "no player $SANDBOX_AS; add one with: tools/sandbox.sh player $SANDBOX_AS"
        echo "$dir/players/$SANDBOX_AS"
    else
        echo "$dir/config"
    fi
}

# Where campaign-edit finds the servers, into `settings`: directly, or
# (SANDBOX_HTTPS=1) all through the proxy, trusting its authority and
# nothing else.
servers() {
    if [ -n "${SANDBOX_HTTPS:-}" ]; then
        [ -f "$dir/https" ] || die "no proxy; start it with: tools/sandbox.sh https"
        local https="https://localhost:$https_port"
        settings=("SSL_CERT_FILE=$root_cert" "CAMPAIGN_SERVER=$https" "CAMPAIGN_COMBAT_SERVER=$https"
            "CAMPAIGN_AUTH_SERVER=$https" "CAMPAIGN_MONSTER_SERVER=$https")
    else
        settings=("CAMPAIGN_SERVER=http://127.0.0.1:$characters_port"
            "CAMPAIGN_COMBAT_SERVER=http://127.0.0.1:$combat_port"
            "CAMPAIGN_AUTH_SERVER=http://127.0.0.1:$auth_port"
            "CAMPAIGN_MONSTER_SERVER=http://127.0.0.1:$monsters_port")
    fi
}
root_cert="$dir/caddy/caddy/pki/authorities/local/root.crt"

# campaign-edit, in the sandbox and nowhere else.
edit_env() {
    local settings
    servers
    env CAMPAIGN_CONFIG_DIR="$(config)" "${settings[@]}" "$@"
}

require_up() {
    [ -f "$dir/ready" ] || die "no sandbox is up; start one with: tools/sandbox.sh up"
}

wait_for() { # url
    for _ in $(seq 120); do
        curl -fs "$1" >/dev/null 2>&1 && return 0
        sleep 0.5
    done
    die "nothing answers at $1 (logs in $dir)"
}

start() { # name, then the command
    local name=$1
    shift
    "$@" >"$dir/$name.log" 2>&1 &
    echo $! >>"$dir/pids"
}

up() {
    [ -e "$dir" ] && die "a sandbox is already up (or left over); tools/sandbox.sh down first"
    for port in $pg_port $characters_port $combat_port $auth_port $monsters_port; do
        if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
            die "port $port is in use"
        fi
    done
    (cd "$root" && cargo build --workspace --quiet)
    mkdir -p "$dir"
    local pg_url="postgres://postgres:postgres@127.0.0.1:$pg_port"

    "$container" run --rm -d --name "$pg" -e POSTGRES_PASSWORD=postgres \
        -p "127.0.0.1:$pg_port:5432" docker.io/library/postgres:17-alpine >/dev/null
    until "$container" exec "$pg" pg_isready -q -h 127.0.0.1 2>/dev/null; do sleep 0.5; done

    DATABASE_URL="$pg_url/auth" start auth "$bin/auth-service" serve --bind "127.0.0.1:$auth_port"
    wait_for "http://127.0.0.1:$auth_port/health"
    local auth="http://127.0.0.1:$auth_port"
    DATABASE_URL="$pg_url/auth" "$bin/auth-service" add-service combat-service \
        "$(SERVICE_KEY_FILE="$dir/combat.key" "$bin/combat-service" public-key 2>/dev/null)" >/dev/null

    DATABASE_URL="sqlite://$dir/characters.db" BIND_ADDR="127.0.0.1:$characters_port" \
        AUTH_SERVICE_URL="$auth" start characters "$bin/character-service"
    for kind in monsters spells conditions; do
        for file in "$root"/data/"$kind"/*.json; do
            [ -e "$file" ] || continue
            DATABASE_URL="sqlite://$dir/monsters.db" "$bin/monster-service" import --kind "$kind" "$file" >/dev/null
        done
    done
    DATABASE_URL="sqlite://$dir/monsters.db" AUTH_SERVICE_URL="$auth" \
        start monsters "$bin/monster-service" serve --bind "127.0.0.1:$monsters_port"
    DATABASE_URL="$pg_url/combats" BIND_ADDR="127.0.0.1:$combat_port" AUTH_SERVICE_URL="$auth" \
        CHARACTER_SERVICE_URL="http://127.0.0.1:$characters_port" \
        MONSTER_SERVICE_URL="http://127.0.0.1:$monsters_port" SERVICE_KEY_FILE="$dir/combat.key" \
        start combat "$bin/combat-service"
    for port in $characters_port $monsters_port $combat_port; do
        wait_for "http://127.0.0.1:$port/health"
    done

    local code
    code=$(DATABASE_URL="$pg_url/auth" "$bin/auth-service" new-campaign Sandbox \
        --edition "${SANDBOX_EDITION:-2024}" | sed -n 's/^    \(.*\)$/\1/p')
    edit_env "$bin/campaign-edit" auth setup "$code" --name "Sandbox DM" >/dev/null
    local characters=("$root"/data/characters/*.json)
    if [ -e "${characters[0]}" ]; then
        edit_env "$bin/campaign-edit" import "${characters[@]}" >/dev/null
    fi
    touch "$dir/ready"
    echo "Sandbox up: campaign Sandbox (${SANDBOX_EDITION:-2024} rules), logs in $dir"
}

run() {
    require_up
    edit_env "$bin/campaign-edit" "$@"
}

session() { # name, then campaign-edit's arguments
    require_up
    [ $# -ge 1 ] || die "usage: tools/sandbox.sh tmux <name> [campaign-edit arguments]"
    local name=$1
    shift
    local command settings
    servers
    command=$(printf '%q ' env CAMPAIGN_CONFIG_DIR="$(config)" "${settings[@]}" \
        "$bin/campaign-edit" "$@")
    tmux new-session -d -s "$name" -x "${COLUMNS:-110}" -y "${LINES:-46}" "$command; sleep 3600"
    echo "$name" >>"$dir/sessions"
}

player() { # name
    require_up
    [ $# -eq 1 ] || die "usage: tools/sandbox.sh player <name>"
    local name=$1
    [ -e "$dir/players/$name" ] && die "there's already a player $name"
    mkdir -p "$dir/players/$name"
    local key campaign
    key=$(SANDBOX_AS=$name edit_env "$bin/campaign-edit" auth key 2>/dev/null)
    campaign=$(edit_env "$bin/campaign-edit" auth status | sed -n 's/^Campaign .*(\(.*\))$/\1/p')
    edit_env "$bin/campaign-edit" auth approve "$key" --name "$name" >/dev/null
    SANDBOX_AS=$name edit_env "$bin/campaign-edit" auth join "$campaign" >/dev/null
    echo "Player $name joined; act as them with SANDBOX_AS=$name"
}

https() {
    require_up
    [ -f "$dir/https" ] && die "the proxy is already up"
    for port in $https_port $http_port; do
        if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then
            die "port $port is in use"
        fi
    done
    mkdir -p "$dir/caddy"
    "$container" run --rm -d --name "$proxy" --network host \
        -e CAMPAIGN_HOST="localhost:$https_port" -e CADDY_HTTP_PORT="$http_port" -e CADDY_ADMIN=off \
        -e CHARACTERS_UPSTREAM="127.0.0.1:$characters_port" -e COMBAT_UPSTREAM="127.0.0.1:$combat_port" \
        -e AUTH_UPSTREAM="127.0.0.1:$auth_port" -e MONSTERS_UPSTREAM="127.0.0.1:$monsters_port" \
        -v "$root/proxy/Caddyfile:/etc/caddy/Caddyfile:ro" -v "$dir/caddy:/data" \
        docker.io/library/caddy:2.10-alpine >/dev/null
    for _ in $(seq 60); do
        [ -f "$root_cert" ] && curl -fs --cacert "$root_cert" "https://localhost:$https_port/health" >/dev/null 2>&1 && break
        sleep 0.5
    done
    curl -fs --cacert "$root_cert" "https://localhost:$https_port/health" >/dev/null ||
        die "the proxy doesn't answer ($container logs $proxy)"
    touch "$dir/https"
    echo "HTTPS proxy up at https://localhost:$https_port; use it with SANDBOX_HTTPS=1"
}

status() {
    if [ -f "$dir/ready" ]; then
        echo "up, in $dir"
        run auth status
    else
        echo "down"
    fi
}

down() {
    if [ -f "$dir/sessions" ]; then
        while read -r name; do tmux kill-session -t "$name" 2>/dev/null || true; done <"$dir/sessions"
    fi
    if [ -f "$dir/pids" ]; then
        while read -r pid; do kill "$pid" 2>/dev/null || true; done <"$dir/pids"
    fi
    "$container" stop "$proxy" >/dev/null 2>&1 || true
    "$container" stop "$pg" >/dev/null 2>&1 || true
    rm -rf "$dir"
    echo "Sandbox down and deleted"
}

case "${1:-}" in
    up) up ;;
    run) shift; run "$@" ;;
    tmux) shift; session "$@" ;;
    player) shift; player "$@" ;;
    https) https ;;
    status) status ;;
    down) down ;;
    *) sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
