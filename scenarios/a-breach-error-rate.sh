#!/usr/bin/env bash
# a-breach-error-rate (plan 167): sustained >20% error rate on checkout for
# >=3 minutes, driven by the explicit provider-decline token. The failure
# remains isolated to this scenario; no normal checkout flag is repurposed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE="${CHECKOUT_URL:-http://localhost:8088}"
BREACH_SECONDS="${BREACH_SECONDS:-200}"
REQUEST_GAP_SECONDS="${REQUEST_GAP_SECONDS:-2}"
COMPOSE=(-f "$ROOT/deploy/docker-compose.yml")

echo "a-breach-error-rate: start stack"
docker compose "${COMPOSE[@]}" up -d flagd pricing inventory recommendation checkout >/dev/null

echo "a-breach-error-rate: driving failing checkout traffic for ${BREACH_SECONDS}s"
end=$((SECONDS + BREACH_SECONDS))
count=0
while ((SECONDS < end)); do
  code="$(curl --max-time 10 -sS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_decline\",\"payment_method_type\":\"card\",\"request_id\":\"breach-$count-$$\"}" \
    -o /dev/null -w "%{http_code}" || true)"
  count=$((count + 1))
  echo "breach request #$count [$code] ($((end - SECONDS))s remaining)"
  sleep "$REQUEST_GAP_SECONDS"
done

echo "a-breach-error-rate: done — decline traffic stopped; run a-recover for healthy traffic."
echo "Check in Parallax UI: Alerts — a high-error-rate rule scoped to checkout opens an incident."
