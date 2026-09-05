#!/usr/bin/env bash
# A4: authenticated seeded-order replay. Java `fulfillment` loads a seeded
# order, publishes it to the durable RabbitMQ exchange, consumes it (CONSUMER
# span + link), then calls Rust `notifications` over HTTP. This is a real
# shared-exchange path, but it is a replay seam, not the checkout transaction's
# outbox proof (a1/a3).
set -euo pipefail
BASE="${FULFILLMENT_URL:-http://localhost:8093}"
FULFILLMENT_TOKEN="${FULFILLMENT_INTERNAL_TOKEN:-fulfillment-internal:research-secret}"

tenant_for_order() {
  case "$1" in
    order-acme-1001) printf 'tenant-acme\n' ;;
    order-nova-2001) printf 'tenant-nova\n' ;;
    *) echo "unknown seeded order: $1" >&2; return 1 ;;
  esac
}

wait_for_fulfillment() {
  local order="$1"
  local tenant
  tenant="$(tenant_for_order "$order")"
  local timeout="${ASYNC_VERIFY_TIMEOUT_SECONDS:-30}"
  local deadline=$((SECONDS + timeout))
  local body=''
  while (( SECONDS < deadline )); do
    if ! body="$(curl --fail-with-body --max-time 5 -sS \
      -H "Authorization: Bearer $FULFILLMENT_TOKEN" \
      -H "X-Tenant-Id: $tenant" \
      "$BASE/verify/order?order=$order&tenant=$tenant")"; then
      echo "fulfillment verification request failed for $order" >&2
      return 1
    fi
    if jq -e '.ready == true' >/dev/null <<<"$body"; then
      return 0
    fi
    sleep 1
  done
  echo "fulfillment verification timed out for $order: $body" >&2
  return 1
}
for order in order-acme-1001 order-nova-2001; do
  tenant="$(tenant_for_order "$order")"
  curl --fail-with-body --max-time 20 -sS -X POST \
    -H "Authorization: Bearer $FULFILLMENT_TOKEN" \
    -H "X-Tenant-Id: $tenant" \
    "$BASE/publish?order=$order&tenant=$tenant" -w " [%{http_code}]\n"
  wait_for_fulfillment "$order"
done
echo "A4 verified — fulfillment(Java) → RabbitMQ → fulfillment(Java) → notifications(Rust)."
