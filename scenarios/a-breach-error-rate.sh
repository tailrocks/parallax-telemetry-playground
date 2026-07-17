#!/usr/bin/env bash
# a-breach-error-rate (plan 167): sustained >20% error rate on checkout for
# >=3 minutes, driven by the flagd paymentFailure chaos flag. Every request
# fails while the flag is on, comfortably clearing any 20% threshold; the
# flag is restored on exit so a-recover (or normal traffic) resolves the
# incident afterwards.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${CHECKOUT_URL:-http://localhost:8088}"
BREACH_SECONDS="${BREACH_SECONDS:-200}"
REQUEST_GAP_SECONDS="${REQUEST_GAP_SECONDS:-2}"
SETTLE_SECONDS="${FLAG_SETTLE_SECONDS:-12}"
FLAG_FILE="$ROOT/flags/flagd.json"
BACKUP="$(mktemp)"
COMPOSE=(-f "$ROOT/deploy/docker-compose.yml")

cp "$FLAG_FILE" "$BACKUP"

restore_flags() {
  cp "$BACKUP" "$FLAG_FILE"
  rm -f "$BACKUP"
}
trap restore_flags EXIT

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

echo "a-breach-error-rate: start stack"
docker compose "${COMPOSE[@]}" up -d flagd pricing inventory recommendation checkout >/dev/null

echo "a-breach-error-rate: force paymentFailure on"
set_payment_flag on
sleep "$SETTLE_SECONDS"

echo "a-breach-error-rate: driving failing checkout traffic for ${BREACH_SECONDS}s"
end=$((SECONDS + BREACH_SECONDS))
count=0
while ((SECONDS < end)); do
  code="$(curl --max-time 10 -sS "$BASE/checkout" -o /dev/null -w "%{http_code}" || true)"
  count=$((count + 1))
  echo "breach request #$count [$code] ($((end - SECONDS))s remaining)"
  sleep "$REQUEST_GAP_SECONDS"
done

echo "a-breach-error-rate: done — flag restored on exit."
echo "Check in Parallax UI: Alerts — a high-error-rate rule scoped to checkout opens an incident."
