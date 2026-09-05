#!/usr/bin/env bash
# B5: bounded checkout latency/in-flight request pressure.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
REQUESTS="${B5_REQUESTS:-12}"
DELAY_MS="${B5_DELAY_MS:-300}"
for _ in $(seq 1 "$REQUESTS"); do
  curl --max-time 30 -sS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"delay_ms\":$DELAY_MS,\"request_id\":\"b5-$$-$_\"}" \
    -o /dev/null -w 'delayed request %{time_total}s [%{http_code}]\n'
done

echo "B5 done — inspect checkout latency, active requests, and slow request spans."
