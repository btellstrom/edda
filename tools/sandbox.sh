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
#   tools/sandbox.sh status             what's running
#   tools/sandbox.sh down               stop it and delete everything in it
#
# `run` and `tmux` always use this checkout's target/debug/campaign-edit,
# with the sandbox's config and servers, and refuse to run without a
# sandbox up.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
dir="$root/target/sandbox"
bin="$root/target/debug"
container=${CONTAINER:-podman}
pg=campaign-manager-sandbox-pg
pg_port=25432
characters_port=23000 combat_port=23001 auth_port=23002 monsters_port=23003

die() { echo "sandbox: $*" >&2; exit 1; }

# campaign-edit, in the sandbox and nowhere else.
edit_env() {
    env CAMPAIGN_CONFIG_DIR="$dir/config" \
        CAMPAIGN_SERVER="http://127.0.0.1:$characters_port" \
        CAMPAIGN_COMBAT_SERVER="http://127.0.0.1:$combat_port" \
        CAMPAIGN_AUTH_SERVER="http://127.0.0.1:$auth_port" \
        CAMPAIGN_MONSTER_SERVER="http://127.0.0.1:$monsters_port" \
        "$@"
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
        CHARACTER_SERVICE_URL="http://127.0.0.1:$characters_port" SERVICE_KEY_FILE="$dir/combat.key" \
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
    local command
    command=$(printf '%q ' env \
        CAMPAIGN_CONFIG_DIR="$dir/config" \
        CAMPAIGN_SERVER="http://127.0.0.1:$characters_port" \
        CAMPAIGN_COMBAT_SERVER="http://127.0.0.1:$combat_port" \
        CAMPAIGN_AUTH_SERVER="http://127.0.0.1:$auth_port" \
        CAMPAIGN_MONSTER_SERVER="http://127.0.0.1:$monsters_port" \
        "$bin/campaign-edit" "$@")
    tmux new-session -d -s "$name" -x "${COLUMNS:-110}" -y "${LINES:-46}" "$command; sleep 3600"
    echo "$name" >>"$dir/sessions"
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
    "$container" stop "$pg" >/dev/null 2>&1 || true
    rm -rf "$dir"
    echo "Sandbox down and deleted"
}

case "${1:-}" in
    up) up ;;
    run) shift; run "$@" ;;
    tmux) shift; session "$@" ;;
    status) status ;;
    down) down ;;
    *) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
