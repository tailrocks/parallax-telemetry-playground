#!/usr/bin/env bash
# B4 provider-unavailable degradation, B13 slow recommendation,
# B16 = loadgen/checkout.ts (k6).
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
echo "B4 degrade:"
curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_unavailable\",\"payment_method_type\":\"card\",\"degrade\":true,\"request_id\":\"b4-$$\"}" \
  -o /dev/null -w " [%{http_code}]\n"
echo "B18 delayed dependency:"
curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"delay_ms\":500,\"request_id\":\"b18-$$\"}" \
  -o /dev/null -w " [%{http_code}]\n"
echo "B4/B18 done — inspect provider-unavailable degraded handling and delayed checkout traces."
