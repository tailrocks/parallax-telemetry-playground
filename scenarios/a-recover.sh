#!/usr/bin/env bash
# a-recover (plan 167): breach recovery — drive sustained healthy traffic so
# open error-rate/latency incidents resolve and the resolved notification fires.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
RECOMMENDATION_BASE="${RECOMMENDATION_URL:-http://localhost:8090}"
RECOVER_SECONDS="${RECOVER_SECONDS:-200}"
REQUEST_GAP_SECONDS="${REQUEST_GAP_SECONDS:-2}"
SETTLE_SECONDS="${FLAG_SETTLE_SECONDS:-12}"
echo "a-recover: waiting for healthy dependencies"
sleep "$SETTLE_SECONDS"

echo "a-recover: driving healthy traffic for ${RECOVER_SECONDS}s"
end=$((SECONDS + RECOVER_SECONDS))
count=0
while ((SECONDS < end)); do
  code="$(curl --max-time 10 -sS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"recover-$count-$$\"}" \
    -o /dev/null -w "%{http_code}" || true)"
  curl --max-time 10 -sS "$RECOMMENDATION_BASE/recommend?tenant_id=tenant-acme&sku=WIDGET-1" -o /dev/null || true
  count=$((count + 1))
  echo "recover request #$count [$code] ($((end - SECONDS))s remaining)"
  sleep "$REQUEST_GAP_SECONDS"
done

echo "a-recover: done."
echo "Check in Parallax UI: Alerts — open incidents resolve; resolved webhook delivered."
