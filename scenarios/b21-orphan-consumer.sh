#!/usr/bin/env bash
set -euo pipefail

BASE="${ORDERS_URL:-http://localhost:8092}"
ORDER_IDENTITY="tenant_id=tenant-acme&customer_id=customer-acme-ava"
TRACE_ID="b7f2d9a4c6e81f03579ab2cd4e6f8102"
ORPHAN_TRACE_ID="c8e3dab5f7a9201468ab3cd5f7a90213"
TRACESTATE="playground=commerce"
BAGGAGE="tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal"

traceparent_for() {
  local trace_id="$1"
  local span_id
  printf -v span_id '%016x' "$2"
  printf '00-%s-%s-01' "$trace_id" "$span_id"
}

post() {
  local label="$1"
  local suffix="$2"
  local traceparent="$3"
  local query="?$ORDER_IDENTITY"
  if [[ -n "$suffix" ]]; then
    query+="&$suffix"
  fi
  curl --fail-with-body --max-time 10 -sS -X POST "$BASE/order$query" \
    -H "traceparent: $traceparent" \
    -H "tracestate: $TRACESTATE" \
    -H "baggage: $BAGGAGE" \
    -o /dev/null -w "$label [%{http_code}]\n"
}

echo "B21 synthetic orders consumer: linked, orphan, and lag burst"
post "linked" "" "$(traceparent_for "$TRACE_ID" 21)"
# orphan=1 makes the service detach its producer span from this valid inbound
# parent; the emitted Rabbit message remains a valid root with orphan marker.
post "orphan" "orphan=1" "$(traceparent_for "$ORPHAN_TRACE_ID" 22)"

pids=()
for i in 1 2 3 4 5 6; do
  curl --fail-with-body --max-time 10 -sS -X POST "$BASE/order?$ORDER_IDENTITY&lag_ms=2000" \
    -H "traceparent: $(traceparent_for "$TRACE_ID" "$((100 + i))")" \
    -H "tracestate: $TRACESTATE" \
    -H "baggage: $BAGGAGE" \
    -o /dev/null -w "lag-$i [%{http_code}]\n" &
  pids+=("$!")
done

for pid in "${pids[@]}"; do
  wait "$pid"
done

sleep 3
echo "Check in Parallax: synthetic orders consumer has a span link; orphan consumer is root/linkless with messaging.orphan=true; messaging.queue.depth gauge rises during lag burst."
