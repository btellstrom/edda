#!/bin/sh
# Once, after the rename from campaign-manager to Edda: copies the old
# stack's volumes into the ones Edda's stack uses, so `make up` finds the
# data. Stop the old stack first (`podman compose -p campaign-manager
# down`). The old volumes are left as they were, to delete once all is well.
#
# CONTAINER=docker for Docker. FROM_PROJECT and COMPOSE_PROJECT_NAME pick
# other stacks (default campaign-manager and edda).
set -eu

CONTAINER=${CONTAINER:-podman}
from=${FROM_PROJECT:-campaign-manager}
to=${COMPOSE_PROJECT_NAME:-edda}
volumes="postgres-data characters-data combat-data monsters-data proxy-data"

die() { echo "adopt-volumes: $*" >&2; exit 1; }

for project in "$from" "$to"; do
    if [ -n "$($CONTAINER ps -q --filter "label=com.docker.compose.project=$project")" ]; then
        die "stack $project is running; stop it first"
    fi
done
found=
for v in $volumes; do
    if $CONTAINER volume inspect "${to}_$v" >/dev/null 2>&1; then
        die "${to}_$v exists already; not copying over it"
    fi
    if $CONTAINER volume inspect "${from}_$v" >/dev/null 2>&1; then
        found="$found $v"
    fi
done
[ -n "$found" ] || die "no volumes of $from to copy"

for v in $found; do
    echo "copying ${from}_$v to ${to}_$v"
    # Labelled as compose labels its own, so it takes the volume as its own.
    $CONTAINER volume create \
        --label "com.docker.compose.project=$to" \
        --label "com.docker.compose.volume=$v" \
        "${to}_$v" >/dev/null
    $CONTAINER run --rm --entrypoint cp \
        -v "${from}_$v:/from:ro" -v "${to}_$v:/to" \
        docker.io/library/postgres:17-alpine -a /from/. /to/
done
echo "done; make up starts the stack on them"
