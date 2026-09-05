#!/usr/bin/env bash
# B7/B8 intentionally exercise the orders service's private synthetic exchange:
# consumer lag and poison retry → dead-letter. This is not checkout outbox
# evidence; a1/a3 cover the real commerce.events outbox path.
set -euo pipefail
BASE="${ORDERS_URL:-http://localhost:8092}"
ORDER_IDENTITY="tenant_id=tenant-acme&customer_id=customer-acme-ava"
TRACE_ID="0af7651916cd43dd8448eb211c80319c"
TRACESTATE="playground=commerce"
BAGGAGE="tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal"

traceparent_for() {
  local span_id
  printf -v span_id '%016x' "$1"
  printf '00-%s-%s-01' "$TRACE_ID" "$span_id"
}

echo "B7 lag:"
curl --fail-with-body --max-time 15 -sS -X POST "$BASE/order?$ORDER_IDENTITY&lag_ms=300" \
  -H "traceparent: $(traceparent_for 7)" \
  -H "tracestate: $TRACESTATE" \
  -H "baggage: $BAGGAGE" \
  -w " [%{http_code}]\n"
echo "B8 poison:"
curl --fail-with-body --max-time 15 -sS -X POST "$BASE/order?$ORDER_IDENTITY&poison=1" \
  -H "traceparent: $(traceparent_for 8)" \
  -H "tracestate: $TRACESTATE" \
  -H "baggage: $BAGGAGE" \
  -w " [%{http_code}]\n"
