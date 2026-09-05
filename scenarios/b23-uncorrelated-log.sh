#!/usr/bin/env bash
# B23: current checkout request with normal trace/log correlation.
set -euo pipefail

BASE="${CHECKOUT_BASE:-http://localhost:8088}"

curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"b23-$$\"}" \
  -o /dev/null -w "checkout [%{http_code}]\n"
sleep 1

echo "B23 done."
echo "Check in Parallax: checkout logs carry the normal trace/span correlation."
