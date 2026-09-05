#!/usr/bin/env bash
# Deliberate-failure catalog (subset) — drive real checkout failure and delay.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
echo "B1 payment decline:"
curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_decline\",\"payment_method_type\":\"card\",\"request_id\":\"b1-$$\"}" \
  -w " [%{http_code}]\n"
echo "B11 latency 500ms:"
curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"slow\":500,\"request_id\":\"b11-$$\"}" \
  -o /dev/null -w "  %{time_total}s [%{http_code}]\n"
echo "done — compare the error grouping + slow-span rendering in each backend UI."
