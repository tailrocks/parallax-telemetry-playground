#!/usr/bin/env bash
# a-recover (plan 167): breach recovery — force the paymentFailure flag off
# and drive sustained healthy traffic so open error-rate/latency incidents
# resolve and the resolved notification fires.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${CHECKOUT_URL:-http://localhost:8088}"
RECOMMENDATION_BASE="${RECOMMENDATION_URL:-http://localhost:8091}"
RECOVER_SECONDS="${RECOVER_SECONDS:-200}"
REQUEST_GAP_SECONDS="${REQUEST_GAP_SECONDS:-2}"
SETTLE_SECONDS="${FLAG_SETTLE_SECONDS:-12}"
FLAG_FILE="$ROOT/flags/flagd.json"

set_payment_flag_off() {
  python3 - "$FLAG_FILE" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as f:
    data = json.load(f)
data["flags"]["paymentFailure"]["defaultVariant"] = "off"
with open(path, "w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY
}

echo "a-recover: force paymentFailure off"
set_payment_flag_off
sleep "$SETTLE_SECONDS"

echo "a-recover: driving healthy traffic for ${RECOVER_SECONDS}s"
end=$((SECONDS + RECOVER_SECONDS))
count=0
while ((SECONDS < end)); do
  code="$(curl --max-time 10 -sS "$BASE/checkout" -o /dev/null -w "%{http_code}" || true)"
  curl --max-time 10 -sS "$RECOMMENDATION_BASE/recommend?sku=WIDGET-1" -o /dev/null || true
  count=$((count + 1))
  echo "recover request #$count [$code] ($((end - SECONDS))s remaining)"
  sleep "$REQUEST_GAP_SECONDS"
done

echo "a-recover: done."
echo "Check in Parallax UI: Alerts — open incidents resolve; resolved webhook delivered."
