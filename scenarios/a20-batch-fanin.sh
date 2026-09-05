#!/usr/bin/env bash
set -euo pipefail

BASE="${ORDERS_URL:-http://localhost:8092}"
TRACE_ID="4bf92f3577b34da6a3ce929d0e0e4736"
TRACESTATE="playground=commerce"
BAGGAGE="tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal"

traceparent_for() {
  local span_id
  printf -v span_id '%016x' "$1"
  printf '00-%s-%s-01' "$TRACE_ID" "$span_id"
}

echo "A20 synthetic fan-in: publish 8 independent orders messages rapidly"
pids=()
for i in 1 2 3 4 5 6 7 8; do
  traceparent="$(traceparent_for "$i")"
  curl --fail-with-body --max-time 10 -sS -X POST "$BASE/order?tenant_id=tenant-acme&customer_id=customer-acme-ava&lag_ms=100" \
    -H "traceparent: $traceparent" \
    -H "tracestate: $TRACESTATE" \
    -H "baggage: $BAGGAGE" \
    -o /dev/null -w "message-$i [%{http_code}]\n" &
  pids+=("$!")
done

for pid in "${pids[@]}"; do
  wait "$pid"
done

sleep 1
echo "Check in Parallax: inspect producer/consumer links and concurrent synthetic orders-queue activity across the 8 messages."
