#!/usr/bin/env bash
# A3: real checkout async branch — checkout commits an order and transactional
# outbox event, the outbox publisher sends commerce.events, and Java fulfillment
# consumes it before notifying Rust. This is the real async path; orders /order
# is an isolated synthetic messaging fixture covered by b-async-chaos/b21.
set -euo pipefail

CHECKOUT_BASE="${CHECKOUT_URL:-http://localhost:8088}"
FULFILLMENT_BASE="${FULFILLMENT_URL:-http://localhost:8093}"
FULFILLMENT_TOKEN="${FULFILLMENT_INTERNAL_TOKEN:-fulfillment-internal:research-secret}"
TENANT_ID="tenant-acme"
CUSTOMER_ID="customer-acme-ava"
REQUEST_ID="a3-outbox-$$"
RESPONSE="$(mktemp)"
trap 'rm -f "$RESPONSE"' EXIT

curl --fail-with-body --max-time 30 -sS -X POST "$CHECKOUT_BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"$TENANT_ID\",\"customer_id\":\"$CUSTOMER_ID\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"$REQUEST_ID\"}" \
  -o "$RESPONSE"

ORDER_ID="$(jq -er '.order_id | strings | select(length > 0)' "$RESPONSE")"
deadline=$((SECONDS + ${ASYNC_VERIFY_TIMEOUT_SECONDS:-30}))
while (( SECONDS < deadline )); do
  if body="$(curl --fail-with-body --max-time 5 -sS \
    -H "Authorization: Bearer $FULFILLMENT_TOKEN" \
    -H "X-Tenant-Id: $TENANT_ID" \
    "$FULFILLMENT_BASE/verify/order?order=$ORDER_ID&tenant=$TENANT_ID")"; then
    if jq -e '.ready == true and .fulfillment_status == "completed" and (.notification_deliveries >= 1)' <<<"$body" >/dev/null; then
      echo "A3 verified — checkout order $ORDER_ID reached fulfillment through the transactional outbox."
      exit 0
    fi
  else
    echo "fulfillment verification request failed for $ORDER_ID" >&2
    exit 1
  fi
  sleep 1
done

echo "fulfillment did not complete checkout order $ORDER_ID before the timeout" >&2
exit 1
