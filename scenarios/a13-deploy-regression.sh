#!/usr/bin/env bash
# A13: release-attributed regression. Recreate checkout with RELEASE=v1,
# then RELEASE=v2; v2 fails through checkout's release branch, no ?fail crutch.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${CHECKOUT_URL:-http://localhost:8088}"
REQUESTS="${A13_REQUESTS:-5}"
A13_BUILD="${A13_BUILD:-0}"
COMPOSE=(-f "$ROOT/deploy/docker-compose.yml")

compose() {
  docker compose "${COMPOSE[@]}" "$@"
}

wait_checkout() {
  for _ in $(seq 1 30); do
    code="$(curl --max-time 10 -sS "$BASE/checkout" -o /dev/null -w "%{http_code}" || true)"
    if [[ "$code" != "000" ]]; then
      return 0
    fi
    sleep 1
  done
  echo "checkout did not become reachable at $BASE" >&2
  return 1
}

drive_burst() {
  local label="$1"
  local expected="$2"
  for i in $(seq 1 "$REQUESTS"); do
    code="$(curl --max-time 10 -sS "$BASE/checkout" -o /dev/null -w "%{http_code}")"
    echo "$label #$i [$code]"
    [[ "$code" == "$expected" ]]
  done
}

restore_v1() {
  echo
  echo "restoring checkout to RELEASE=v1"
  RELEASE=v1 compose up -d --no-deps --force-recreate checkout >/dev/null
}

start_v1_stack() {
  local args=(up -d)
  if [[ "$A13_BUILD" == "1" || "$A13_BUILD" == "true" ]]; then
    args+=(--build)
  fi
  args+=(flagd pricing inventory recommendation checkout)
  RELEASE=v1 compose "${args[@]}" >/dev/null
}

trap restore_v1 EXIT

echo "A13 phase 1: checkout RELEASE=v1 clean"
start_v1_stack
wait_checkout
drive_burst "v1 clean" 200

echo
echo "A13 phase 2: checkout RELEASE=v2 regressed"
RELEASE=v2 compose up -d --no-deps --force-recreate checkout >/dev/null
wait_checkout
drive_burst "v2 regressed" 502

restore_v1
trap - EXIT

echo
echo "Check in Parallax UI:"
echo "- Issues: checkout error spike attributed to service.version=v2"
echo "- Services -> checkout: release strip shows v1 -> v2 once plan 041 lands"
