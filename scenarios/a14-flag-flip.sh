#!/usr/bin/env bash
# A14: live flagd flip. checkoutFlow control -> orchestrated -> control,
# without restarting checkout. Both variants are real successful journeys;
# the response and trace expose the selected topology variant.
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

set_checkout_variant() {
  local variant="$1"
  python3 - "$FLAG_FILE" "$variant" <<'PY'
import json
import sys

path, variant = sys.argv[1], sys.argv[2]
with open(path, encoding="utf-8") as f:
    data = json.load(f)
data["flags"]["checkoutFlow"]["defaultVariant"] = variant
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

wait_checkout() {
  for _ in $(seq 1 30); do
    code="$(curl --max-time 10 -sS "$BASE/healthz" -o /dev/null -w "%{http_code}" || true)"
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
  local expected_variant="$2"
  for i in $(seq 1 "$REQUESTS"); do
    body="$(curl --max-time 20 -sS -X POST "$BASE/checkout" \
      -H 'content-type: application/json' \
      --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"a14-$expected_variant-$i-$$\"}")"
    code="$?"
    printf '%s\n' "${label} #${i} [curl=${code}] ${body:0:500}"
    [[ "$code" == "0" ]]
    [[ "$body" == *"\"feature_variant\":\"$expected_variant\""* ]]
  done
}

trap restore_flags EXIT

echo "A14 start stack"
RELEASE=v1 compose up -d flagd pricing inventory recommendation checkout >/dev/null
wait_checkout

echo "A14 force checkoutFlow=control"
set_checkout_variant control
sleep "$SETTLE_SECONDS"
drive_burst "control" control

echo
echo "A14 flip checkoutFlow=orchestrated"
set_checkout_variant orchestrated
sleep "$SETTLE_SECONDS"
drive_burst "orchestrated" orchestrated

echo
echo "A14 flip checkoutFlow=control"
set_checkout_variant control
sleep "$SETTLE_SECONDS"
drive_burst "control again" control

echo
echo "Check in Parallax UI:"
echo "- Trace detail: checkout spans include feature_flag.evaluation events"
echo "- Checkout response/trace: feature_variant changes live with no restart"
