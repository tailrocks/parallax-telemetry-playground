#!/usr/bin/env bash
# B10: concurrent checkout requests through the current bounded delay path.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
REQUESTS="${B10_REQUESTS:-12}"
for i in $(seq 1 "$REQUESTS"); do
  curl --max-time 30 -sS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"delay_ms\":150,\"request_id\":\"b10-$i-$$\"}" \
    -o /dev/null -w "request $i %{time_total}s [%{http_code}]\n" &
done
wait
echo "B10 done — inspect concurrent checkout spans and bounded delay."
