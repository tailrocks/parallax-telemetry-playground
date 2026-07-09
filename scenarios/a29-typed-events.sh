#!/usr/bin/env bash
set -euo pipefail

CHECKOUT_URL="${CHECKOUT_URL:-http://localhost:8088}"
ORDERS_URL="${ORDERS_URL:-http://localhost:8092}"
CATALOG_URL="${CATALOG_URL:-http://localhost:8080}"
GRAPHQL="$CATALOG_URL/graphql"

request() {
  local label="$1"
  local method="$2"
  local url="$3"
  local expected="$4"
  local code
  code="$(curl --max-time 15 -sS -X "$method" "$url" -o /dev/null -w "%{http_code}")"
  printf "%-24s [%s]\n" "$label" "$code"
  if [[ "$code" != "$expected" ]]; then
    echo "expected $expected for $label, got $code" >&2
    exit 1
  fi
}

graphql_products() {
  local outfile
  outfile="$(mktemp)"
  local code
  code="$(curl --max-time 15 -sS "$GRAPHQL" \
    -H 'content-type: application/json' \
    --data '{"query":"query typedEventsProducts { products { id sku name } }"}' \
    -o "$outfile" -w "%{http_code}")"
  printf "%-24s [%s]\n" "catalog products" "$code"
  if [[ "$code" != "200" ]]; then
    cat "$outfile" >&2
    rm -f "$outfile"
    exit 1
  fi
  rm -f "$outfile"
}

echo "A29 typed events: Rust, Java, and web/business structured logs"
request "checkout completed" GET "$CHECKOUT_URL/checkout?sku=WIDGET-1&quantity=2" 200
request "checkout failed" GET "$CHECKOUT_URL/checkout?sku=RUM-ERROR&quantity=1&fail=1" 502
request "order consumed" POST "$ORDERS_URL/order" 200
graphql_products

cat <<CHECKS

Check in Parallax:
  - Native GreptimeDB logs table:
    SELECT json_get_string(log_attributes, 'event.name') AS event_name,
      body, log_attributes FROM opentelemetry_logs
    WHERE json_get_string(log_attributes, 'event.name') IN ('checkout.completed', 'checkout.failed', 'order.consumed',
      'catalog.products.served', 'payment.authorized', 'web.checkout.submitted');
  - Expected typed event names:
    checkout.completed, checkout.failed, order.consumed, catalog.products.served.
  - With deploy/docker-compose.xlang.yml enabled for checkout, also expect:
    payment.authorized.
  - The web checkout page emits web.checkout.submitted when submitted from a browser.
CHECKS
