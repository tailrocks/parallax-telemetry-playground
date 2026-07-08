#!/usr/bin/env bash
set -euo pipefail

BASE="${ORDERS_URL:-http://localhost:8092}"

post() {
  local label="$1"
  local query="$2"
  curl --max-time 10 -sS -X POST "$BASE/order$query" \
    -o /dev/null -w "$label [%{http_code}]\n"
}

echo "B21 orphan consumer: linked, orphan, and lag burst"
post "linked" ""
post "orphan" "?orphan=1"

pids=()
for i in 1 2 3 4 5 6; do
  curl --max-time 10 -sS -X POST "$BASE/order?lag_ms=2000" \
    -o /dev/null -w "lag-$i [%{http_code}]\n" &
  pids+=("$!")
done

for pid in "${pids[@]}"; do
  wait "$pid"
done

sleep 3
echo "Check in Parallax: linked consumer has a span link; orphan consumer is root/linkless with messaging.orphan=true; messaging.queue.depth gauge rises during lag burst."
