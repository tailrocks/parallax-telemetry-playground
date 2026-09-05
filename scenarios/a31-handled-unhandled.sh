#!/usr/bin/env bash
# A31: compare current handled payment decline (402) and provider-internal (502) outcomes.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
handled_file="$(mktemp)"
internal_file="$(mktemp)"
trap 'rm -f "$handled_file" "$internal_file"' EXIT
handled="$(curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data '{"tenant_id":"tenant-acme","customer_id":"customer-acme-ava","items":[{"sku":"WIDGET-1","quantity":1}],"currency_code":"USD","payment_method_token":"tok_decline","payment_method_type":"card","request_id":"a31-decline"}' \
  -o "$handled_file" -w "%{http_code}")"
internal="$(curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data '{"tenant_id":"tenant-acme","customer_id":"customer-acme-ava","items":[{"sku":"WIDGET-1","quantity":1}],"currency_code":"USD","payment_method_token":"tok_internal","payment_method_type":"card","request_id":"a31-internal"}' \
  -o "$internal_file" -w "%{http_code}")"
if [[ "$handled" != "402" ]]; then
  echo "A31: expected handled decline 402, got $handled" >&2
  exit 1
fi
if [[ "$internal" != "502" ]]; then
  echo "A31: expected provider-internal 502, got $internal" >&2
  exit 1
fi
echo "A31 done — handled provider decline $handled vs handled provider internal $internal."
