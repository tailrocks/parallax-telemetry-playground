#!/usr/bin/env bash
# B3 timeout/retry and bounded slow pricing path.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
echo "B3 retry=2 timeout=50ms with delayed pricing:"
curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"retry\":2,\"timeout_ms\":50,\"delay_ms\":350,\"request_id\":\"b3-$$\"}" \
  -o /dev/null -w " [%{http_code}]\n"
echo "B9 slow pricing path:"
curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"delay_ms\":400,\"request_id\":\"b9-$$\"}" \
  -o /dev/null -w " [%{http_code}]\n"
