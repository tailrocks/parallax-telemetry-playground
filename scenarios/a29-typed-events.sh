#!/usr/bin/env bash
set -euo pipefail

CHECKOUT_URL="${CHECKOUT_URL:-http://localhost:8088}"
ORDERS_URL="${ORDERS_URL:-http://localhost:8092}"
CATALOG_URL="${CATALOG_URL:-http://localhost:8080}"
TENANT_ID="tenant-acme"
GRAPHQL="$CATALOG_URL/graphql"
TRACE_ID="d4c3b2a19087654321fedcba98765432"
TRACESTATE="playground=commerce"
BAGGAGE="tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal"

traceparent_for() {
  local span_id
  printf -v span_id '%016x' "$1"
  printf '00-%s-%s-01' "$TRACE_ID" "$span_id"
}

request() {
  local label="$1"
  local method="$2"
  local url="$3"
  local expected="$4"
  local body="${5:-}"
  local producer_id="${6:-1}"
  local code
  local args=(--max-time 15 -sS -X "$method" "$url"
    -H "traceparent: $(traceparent_for "$producer_id")"
    -H "tracestate: $TRACESTATE"
    -H "baggage: $BAGGAGE"
    -o /dev/null -w "%{http_code}")
  if [[ "$expected" == 2* ]]; then
    args+=(--fail-with-body)
  fi
  if [[ -n "$body" ]]; then
    args+=(-H 'content-type: application/json' --data "$body")
  fi
  code="$(curl "${args[@]}")"
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
    -H "x-tenant-id: $TENANT_ID" \
    --data '{"query":"query typedEventsProducts { products { items { id sku name } } }"}' \
    -o "$outfile" -w "%{http_code}")"
  printf "%-24s [%s]\n" "catalog products" "$code"
  if [[ "$code" != "200" ]]; then
    cat "$outfile" >&2
    rm -f "$outfile"
    exit 1
  fi
  if ! jq -e '
    ((.errors // []) | length == 0)
    and ((.data.products.items // []) | type == "array")
    and (((.data.products.items // []) | length) > 0)
  ' "$outfile" >/dev/null; then
    echo "catalog products response had GraphQL errors or no product data" >&2
    cat "$outfile" >&2
    rm -f "$outfile"
    exit 1
  fi
  rm -f "$outfile"
}

echo "A29 typed events: real checkout outcomes, synthetic orders dispatch, and catalog logs"
request "checkout completed" POST "$CHECKOUT_URL/checkout" 200 '{"tenant_id":"tenant-acme","customer_id":"customer-acme-ava","items":[{"sku":"WIDGET-1","quantity":1}],"currency_code":"USD","payment_method_token":"tok_visa","payment_method_type":"card","request_id":"a29-success"}' 1
request "payment declined" POST "$CHECKOUT_URL/checkout" 402 '{"tenant_id":"tenant-acme","customer_id":"customer-acme-ava","items":[{"sku":"WIDGET-1","quantity":1}],"currency_code":"USD","payment_method_token":"tok_decline","payment_method_type":"card","request_id":"a29-decline"}' 2
request "synthetic order dispatch" POST "$ORDERS_URL/order?tenant_id=tenant-acme&customer_id=customer-acme-ava" 200 '' 3
graphql_products

cat <<'CHECKS'

Check in Parallax:
  - Native GreptimeDB logs table:
    SELECT json_get_string(log_attributes, 'event.name') AS event_name,
      body, log_attributes FROM opentelemetry_logs
    WHERE json_get_string(log_attributes, 'event.name') IN ('checkout.completed', 'order.consumed',
      'catalog.products.served', 'web.checkout.submitted');
  - Expected typed event names:
    checkout.completed, order.consumed, catalog.products.served.
  - `order.consumed` is emitted by the isolated synthetic orders fixture;
    checkout outbox/fulfillment evidence comes from `a1` and `a3`.
  - The web checkout page emits web.checkout.submitted when submitted from a browser.
CHECKS
