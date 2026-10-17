#!/usr/bin/env sh
# Runs a command (the tests) with a throwaway Postgres: started on a port of
# its own, passed in TEST_POSTGRES_URL, and deleted afterwards, pass or
# fail. A real stack's Postgres never sees test data.
#
#   tools/with-test-postgres.sh cargo test --workspace
#
# With TEST_POSTGRES_URL already set (as in CI), just runs the command.
set -eu

if [ -n "${TEST_POSTGRES_URL:-}" ]; then
    exec "$@"
fi
CONTAINER=${CONTAINER:-podman}
name=campaign-manager-test-pg-$$
port=${TEST_POSTGRES_PORT:-55432}
"$CONTAINER" run --rm -d --name "$name" -e POSTGRES_PASSWORD=postgres \
    -p "127.0.0.1:$port:5432" docker.io/library/postgres:17-alpine >/dev/null
trap '"$CONTAINER" stop "$name" >/dev/null' EXIT
until "$CONTAINER" exec "$name" pg_isready -q -h 127.0.0.1 2>/dev/null; do sleep 0.5; done
TEST_POSTGRES_URL=postgres://postgres:postgres@127.0.0.1:$port "$@"
