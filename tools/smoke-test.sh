#!/usr/bin/env sh
# End-to-end check of the real thing: builds the server image, starts the
# whole compose stack as a separate project (its own image, volumes and
# ports, so a running stack is left alone), seeds it, and drives
# campaign-edit through setting up a campaign, importing characters and
# looking things up. Deletes all of it afterwards, pass or fail.
#
#   tools/smoke-test.sh            (CONTAINER=docker for Docker)
#
# Needs the SRD data (make srd) and a built campaign-edit
# (CAMPAIGN_EDIT, default target/release/campaign-edit).
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
CONTAINER=${CONTAINER:-podman}
EDIT=${CAMPAIGN_EDIT:-"$here/target/release/campaign-edit"}
export COMPOSE_PROJECT_NAME=campaign-manager-smoke
export IMAGE=localhost/campaign-manager-smoke
export POSTGRES_PORT=15432 CHARACTERS_PORT=13000 COMBAT_PORT=13001 AUTH_PORT=13002 MONSTERS_PORT=13003
compose() { "$CONTAINER" compose "$@"; }

work=$(mktemp -d)
dangling() { "$CONTAINER" images --quiet --filter dangling=true | sort -u; }
dangling > "$work/before"
cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then compose logs --tail 50 || true; fi
    compose down --volumes --remove-orphans >/dev/null 2>&1 || true
    "$CONTAINER" rmi "$IMAGE" >/dev/null 2>&1 || true
    # The build's leftover layers, and only those: the build stage alone
    # is well over a gigabyte.
    dangling | comm -13 "$work/before" - | xargs -r "$CONTAINER" rmi >/dev/null 2>&1 || true
    rm -rf "$work"
    exit "$status"
}
trap cleanup EXIT

export CAMPAIGN_CONFIG_DIR="$work/config"
export CAMPAIGN_SERVER=http://127.0.0.1:$CHARACTERS_PORT
export CAMPAIGN_COMBAT_SERVER=http://127.0.0.1:$COMBAT_PORT
export CAMPAIGN_AUTH_SERVER=http://127.0.0.1:$AUTH_PORT
export CAMPAIGN_MONSTER_SERVER=http://127.0.0.1:$MONSTERS_PORT

step() { printf '\n== %s\n' "$*"; }
expect() { # what output
    case "$2" in
        *"$1"*) ;;
        *) printf 'expected "%s" in:\n%s\n' "$1" "$2" >&2; exit 1 ;;
    esac
}

step "Starting the stack"
# Whatever an interrupted run left behind goes first.
compose down --volumes --remove-orphans >/dev/null 2>&1 || true
"$CONTAINER" build --network host -t "$IMAGE" .
compose up -d
# The monster service starts once the seed job is done, and the combat
# service once it's registered with auth: when all four answer, it's up.
for port in $AUTH_PORT $CHARACTERS_PORT $MONSTERS_PORT $COMBAT_PORT; do
    tries=0
    until curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; do
        tries=$((tries + 1))
        if [ "$tries" -gt 120 ]; then echo "nothing answers on port $port" >&2; exit 1; fi
        sleep 1
    done
done

step "The seed job loaded the SRD"
seeded=$(compose logs seed-monsters)
for loaded in "monsters/srd-5.2.json: 341 added" "spells/srd-5.2.json: 339 added" "conditions/srd-5.2.json: 15 added"; do
    expect "$loaded" "$seeded"
done

step "Setting up a campaign"
code=$(compose exec -T auth auth-service new-campaign Smoke --edition 2024 | sed -n 's/^    \(.*\)$/\1/p')
"$EDIT" auth setup "$code" --name "Smoke DM" >/dev/null

step "Characters"
cat > "$work/vex.json" <<'JSON'
{"name": "Vex", "classes": [{"name": "Ranger", "level": 5, "spellcasting_ability": "wisdom"}],
 "hit_points": {"current": 44, "max": 44},
 "spellcasting": {"spells": [{"name": "Hunter's Mark", "level": 1, "concentration": true}]}}
JSON
"$EDIT" import "$work/vex.json"
expect "Vex" "$("$EDIT" list)"
expect "Hunter's Mark" "$("$EDIT" vex show)"

step "Monsters, spells and conditions"
expect "Goblin Warrior" "$("$EDIT" monster list goblin)"
expect "Level 3 Evocation" "$("$EDIT" spell show fireball)"
expect "Speed 0" "$("$EDIT" condition show grappled)"

printf '\nSmoke test passed.\n'
