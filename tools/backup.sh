#!/usr/bin/env sh
# Backs up the compose stack's data while it runs: the auth and combat
# databases (pg_dump), the characters and monsters (SQLite copies), and
# the combat service's key. Put it back with tools/restore.sh.
#
#   tools/backup.sh [DIR]       default backups/<date and time>
#
# CONTAINER=docker for Docker; COMPOSE_PROJECT_NAME picks another stack.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
CONTAINER=${CONTAINER:-podman}
compose() { "$CONTAINER" compose "$@"; }
dir=${1:-backups/$(date +%Y-%m-%d-%H%M%S)}
[ -e "$dir" ] && { echo "backup: $dir already exists" >&2; exit 1; }
mkdir -p "$dir"
partial() { echo "backup: failed; $dir is incomplete" >&2; }
trap partial EXIT

for db in auth combats; do
    compose exec -T postgres pg_dump -U postgres --format custom "$db" > "$dir/$db.dump"
done
# A consistent copy made inside the container, then fetched.
for service in characters monsters; do
    copy=/data/backup-$$.db
    compose exec -T "$service" "${service%s}-service" backup "$copy"
    compose exec -T "$service" cat "$copy" > "$dir/$service.db"
    compose exec -T "$service" rm -f "$copy"
done
compose exec -T combat cat /data/combat-service.key > "$dir/combat-service.key"
chmod 600 "$dir/combat-service.key"

trap - EXIT
echo "Backed up to $dir"
