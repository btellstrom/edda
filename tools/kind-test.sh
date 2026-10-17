#!/usr/bin/env sh
# End-to-end check of the Helm chart: builds the server image, starts a
# Kubernetes cluster of its own (kind), with a Postgres of its own, and
# installs the chart there with its admin API, token key and metrics.
# Then creates a campaign through the admin API and drives edda through
# setting it up and adding a character. Deletes all of it afterwards,
# pass or fail; a running stack, and this machine's kube config, are left
# alone.
#
#   tools/kind-test.sh            (CONTAINER=docker for Docker)
#
# Needs kind, kubectl, helm and a built edda (EDDA, default
# target/release/edda).
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
CONTAINER=${CONTAINER:-podman}
EDIT=${EDDA:-"$here/target/release/edda"}
IMAGE=localhost/edda-kind:test
POSTGRES_IMAGE=docker.io/library/postgres:17-alpine
CLUSTER=edda-kind-test
if [ "$CONTAINER" = podman ]; then export KIND_EXPERIMENTAL_PROVIDER=podman; fi

work=$(mktemp -d)
export KUBECONFIG="$work/kubeconfig"
dangling() { "$CONTAINER" images --quiet --filter dangling=true | sort -u; }
dangling > "$work/before"
forwards=""
cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then
        kubectl get pods -o wide 2>/dev/null || true
        kubectl logs -l app.kubernetes.io/name=edda --all-containers --tail 30 --prefix 2>/dev/null || true
    fi
    for pid in $forwards; do kill "$pid" 2>/dev/null || true; done
    kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
    "$CONTAINER" rmi "$IMAGE" >/dev/null 2>&1 || true
    dangling | comm -13 "$work/before" - | xargs -r "$CONTAINER" rmi >/dev/null 2>&1 || true
    rm -rf "$work"
    exit "$status"
}
trap cleanup EXIT

step() { printf '\n== %s\n' "$*"; }
expect() { # what output
    case "$2" in
        *"$1"*) ;;
        *) printf 'expected "%s" in:\n%s\n' "$1" "$2" >&2; exit 1 ;;
    esac
}
# 32 random bytes as Edda writes keys: unpadded base64url, and a newline.
key() { python3 -c 'import base64, os; print(base64.urlsafe_b64encode(os.urandom(32)).decode().rstrip("="))'; }
# Forwards a free local port to `service`'s `port`; prints the local one.
forward() { # service port
    free=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
    kubectl port-forward "svc/$1" "$free:$2" >"$work/forward-$1-$2.log" 2>&1 &
    forwards="$forwards $!"
    tries=0
    until grep -q Forwarding "$work/forward-$1-$2.log" 2>/dev/null; do
        tries=$((tries + 1))
        if [ "$tries" -gt 60 ]; then cat "$work/forward-$1-$2.log" >&2; exit 1; fi
        sleep 0.5
    done
    echo "$free"
}

step "A cluster of its own"
kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true
"$CONTAINER" build --network host -t "$IMAGE" .
kind create cluster --name "$CLUSTER" --wait 180s
for image in "$IMAGE" "$POSTGRES_IMAGE"; do
    "$CONTAINER" image inspect "$image" >/dev/null 2>&1 || "$CONTAINER" pull "$image"
    "$CONTAINER" save "$image" -o "$work/image.tar"
    kind load image-archive --name "$CLUSTER" "$work/image.tar"
    rm "$work/image.tar"
done

step "Postgres, and the secrets the chart reads"
kubectl apply -f - <<YAML
apiVersion: apps/v1
kind: Deployment
metadata: {name: postgres}
spec:
  selector: {matchLabels: {app: postgres}}
  template:
    metadata: {labels: {app: postgres}}
    spec:
      containers:
        - name: postgres
          image: $POSTGRES_IMAGE
          imagePullPolicy: Never
          env: [{name: POSTGRES_PASSWORD, value: postgres}]
          readinessProbe: {exec: {command: [pg_isready, -U, postgres]}, periodSeconds: 2}
---
apiVersion: v1
kind: Service
metadata: {name: postgres}
spec:
  selector: {app: postgres}
  ports: [{port: 5432}]
YAML
url=postgres://postgres:postgres@postgres:5432
kubectl create secret generic edda-database \
    --from-literal=auth="$url/auth" --from-literal=characters="$url/characters" \
    --from-literal=combats="$url/combats" --from-literal=monsters="$url/monsters"
key > "$work/combat-service.key"
key > "$work/token.key"
kubectl create secret generic edda-combat-key --from-file="$work/combat-service.key"
kubectl create secret generic edda-token-key --from-file="$work/token.key"
admin_token=$(key)
kubectl create secret generic edda-admin --from-literal=token="$admin_token"
kubectl rollout status deploy/postgres --timeout 180s

step "The chart"
helm install edda deploy/helm/edda --wait --timeout 5m \
    --set image.repository=localhost/edda-kind --set image.tag=test --set image.pullPolicy=Never \
    --set admin.tokenSecret=edda-admin --set auth.tokenKeySecret=edda-token-key
kubectl get pods

step "A campaign through the admin API"
admin=$(forward edda-auth 9200)
created=$(curl -fsS -X POST "http://127.0.0.1:$admin/admin/campaigns" \
    -H "authorization: Bearer $admin_token" -H 'content-type: application/json' \
    -d '{"name": "Kind", "edition": "2024"}')
expect '"name":"Kind"' "$created"
code=$(printf '%s' "$created" | python3 -c 'import json, sys; print(json.load(sys.stdin)["setup_code"])')

step "Set up and played through edda"
export EDDA_CONFIG_DIR="$work/config"
EDDA_AUTH_SERVER=http://127.0.0.1:$(forward edda-auth 3002)
EDDA_SERVER=http://127.0.0.1:$(forward edda-characters 3000)
export EDDA_AUTH_SERVER EDDA_SERVER
"$EDIT" auth setup "$code" --name "Kind DM" >/dev/null
"$EDIT" new Grog --race Goliath >/dev/null
expect "Grog" "$("$EDIT" list)"

step "Measured, signed with the shared key"
metrics=$(curl -fsS "http://127.0.0.1:$(forward edda-characters 9100)/metrics")
expect 'edda_http_requests_total{service="character service",method="POST",route="/characters",status="201"} 1' "$metrics"
public=$(curl -fsS "$EDDA_AUTH_SERVER/public-key")
# The key's public half, as the image works it out.
expect "$("$CONTAINER" run --rm -v "$work:/keys:ro" -e SERVICE_KEY_FILE=/keys/token.key \
    "$IMAGE" combat-service public-key)" "$public"

printf '\nThe chart works.\n'
