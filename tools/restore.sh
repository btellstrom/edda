#!/usr/bin/env sh
# Puts a backup from tools/backup.sh back into the compose stack: stops
# the services, replaces all their data with the backup's (whatever has
# changed since is lost), and starts them again.
#
#   tools/restore.sh DIR
#
# CONTAINER=docker for Docker; COMPOSE_PROJECT_NAME picks another stack.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
CONTAINER=${CONTAINER:-podman}
compose() { "$CONTAINER" compose "$@"; }
[ $# -eq 1 ] || { echo "usage: tools/restore.sh DIR" >&2; exit 2; }
dir=$(cd "$1" && pwd)
for file in auth.dump characters.dump combats.dump monsters.dump combat-service.key; do
    [ -f "$dir/$file" ] || { echo "restore: $dir has no $file; not a backup?" >&2; exit 1; }
done

services="auth characters combat monsters"
# shellcheck disable=SC2086 # one word per service
compose stop $services
for db in auth characters combats monsters; do
    compose exec -T postgres dropdb -U postgres --if-exists --force "$db"
    compose exec -T postgres createdb -U postgres "$db"
    compose exec -T postgres pg_restore -U postgres --no-owner --dbname "$db" < "$dir/$db.dump"
done
# Into the combat service's volume, with it stopped. Piped in rather than
# mounted: the container's user may not be able to read it where it is
# (it's yours alone).
compose run --rm --no-deps -T combat \
    sh -c "cat > /data/combat-service.key && chmod 600 /data/combat-service.key" \
    < "$dir/combat-service.key"
# shellcheck disable=SC2086
compose start $services
echo "Restored from $dir"
