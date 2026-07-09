#!/usr/bin/env bash
# A14: live flagd flip. paymentFailure off -> healthy checkout, on -> 502,
# off again -> healthy, without restarting checkout.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${CHECKOUT_URL:-http://localhost:8088}"
REQUESTS="${A14_REQUESTS:-5}"
SETTLE_SECONDS="${A14_FLAG_SETTLE_SECONDS:-12}"
FLAG_FILE="$ROOT/flags/flagd.json"
BACKUP="$(mktemp)"
COMPOSE=(-f "$ROOT/deploy/docker-compose.yml")

cp "$FLAG_FILE" "$BACKUP"

compose() {
  docker compose "${COMPOSE[@]}" "$@"
}

restore_flags() {
  cp "$BACKUP" "$FLAG_FILE"
  rm -f "$BACKUP"
}

set_payment_flag() {
  local variant="$1"
  python3 - "$FLAG_FILE" "$variant" <<'PY'
import json
import sys

path, variant = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as f:
    data = json.load(f)
data["flags"]["paymentFailure"]["defaultVariant"] = variant
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
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

trap restore_flags EXIT

echo "A14 start stack"
RELEASE=v1 compose up -d flagd pricing inventory recommendation checkout >/dev/null
wait_checkout

echo "A14 force paymentFailure off"
set_payment_flag off
sleep "$SETTLE_SECONDS"
drive_burst "flag off" 200

echo
echo "A14 flip paymentFailure on"
set_payment_flag on
sleep "$SETTLE_SECONDS"
drive_burst "flag on" 502

echo
echo "A14 flip paymentFailure off"
set_payment_flag off
sleep "$SETTLE_SECONDS"
drive_burst "flag off again" 200

echo
echo "Check in Parallax UI:"
echo "- Trace detail: checkout spans include feature_flag.evaluation events"
echo "- Issues: checkout failures appear only while paymentFailure=on"
