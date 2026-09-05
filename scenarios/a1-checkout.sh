#!/usr/bin/env bash
# A1: real checkout flow → distributed trace (Catalog GraphQL → Pricing gRPC
# → Inventory/Postgres → Payment gRPC → durable outbox).
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
FULFILLMENT_BASE="${FULFILLMENT_URL:-http://localhost:8093}"
FULFILLMENT_TOKEN="${FULFILLMENT_INTERNAL_TOKEN:-fulfillment-internal:research-secret}"
RESPONSE="$(mktemp)"
trap 'rm -f "$RESPONSE"' EXIT

wait_for_fulfillment() {
  local order_id="$1"
  local deadline=$((SECONDS + ${ASYNC_VERIFY_TIMEOUT_SECONDS:-30}))
  local body
  while (( SECONDS < deadline )); do
    if ! body="$(curl --fail-with-body --max-time 5 -sS \
      -H "Authorization: Bearer $FULFILLMENT_TOKEN" \
      -H 'X-Tenant-Id: tenant-acme' \
      "$FULFILLMENT_BASE/verify/order?order=$order_id&tenant=tenant-acme")"; then
      echo "fulfillment verification request failed for $order_id" >&2
      return 1
    fi
    if jq -e '.ready == true and .fulfillment_status == "completed" and (.notification_deliveries >= 1)' <<<"$body" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  echo "fulfillment did not complete checkout order $order_id before the timeout" >&2
  return 1
}

for q in 1 2; do
  curl --fail-with-body --max-time 30 -sS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":$q}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"a1-$q-$$\"}" \
    -o "$RESPONSE"
  cat "$RESPONSE"
  ORDER_ID="$(jq -er '.order_id | strings | select(length > 0)' "$RESPONSE")"
  wait_for_fulfillment "$ORDER_ID"
  echo
done
echo "A1 done — checkout orders reached fulfillment through the transactional outbox; inspect Catalog, Pricing, Inventory, Payment, Postgres, and RabbitMQ spans."
