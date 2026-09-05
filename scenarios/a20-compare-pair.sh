#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${CHECKOUT_URL:-http://localhost:8088}"
FLAG_FILE="$ROOT/flags/flagd.json"
FLAG_BACKUP="$(mktemp)"
SETTLE_SECONDS="${A20_FLAG_SETTLE_SECONDS:-12}"
cp "$FLAG_FILE" "$FLAG_BACKUP"
COMPOSE=(-f "$ROOT/deploy/docker-compose.yml")

restore_flag() {
  cp "$FLAG_BACKUP" "$FLAG_FILE"
  rm -f "$FLAG_BACKUP"
}
trap restore_flag EXIT

set_checkout_variant() {
  local variant="$1"
  python3 - "$FLAG_FILE" "$variant" <<'PY'
import json
import pathlib
import sys

path, variant = sys.argv[1:]
data = json.loads(pathlib.Path(path).read_text())
data["flags"]["checkoutFlow"]["defaultVariant"] = variant
pathlib.Path(path).write_text(json.dumps(data, indent=2) + "\n")
PY
}

wait_checkout() {
  for _ in $(seq 1 30); do
    if curl --max-time 5 -sS "$BASE/healthz" -o /dev/null; then return 0; fi
    sleep 1
  done
  echo "checkout did not become reachable at $BASE" >&2
  return 1
}

run_variant() {
  local variant="$1"
  local body
  set_checkout_variant "$variant"
  docker compose "${COMPOSE[@]}" up -d flagd pricing inventory recommendation checkout >/dev/null
  wait_checkout
  sleep "$SETTLE_SECONDS"
  body="$(curl --fail-with-body --max-time 15 -sS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"a20-${variant}\"}")"
  [[ "$body" == *"\"feature_variant\":\"$variant\""* ]] || {
    echo "A20 $variant response did not report feature_variant=$variant: $body" >&2
    return 1
  }
  echo "$variant: $body"
}

echo "A20 checkoutFlow structural compare pair"
run_variant control
run_variant orchestrated
echo "Check in Parallax: control omits recommendation; orchestrated includes it. Compare the two checkout traces and feature_variant attributes."
