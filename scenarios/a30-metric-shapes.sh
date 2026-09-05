#!/usr/bin/env bash
# A30: exercise the current checkout request metrics and latency paths.
# Drives valid seeded checkout traffic so http.server.active_requests and
# normal service metrics are emitted.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
index=0
for sku in WIDGET-1 WIDGET-2 GADGET-1 WIDGET-1; do
  index=$((index + 1))
  curl -fsS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"${sku}\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"a30-${sku}-${index}-$$\"}" \
    >/dev/null
done
echo "A30 done — inspect http.server.active_requests, request latency, and normal checkout service metrics."
