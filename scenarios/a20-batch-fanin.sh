#!/usr/bin/env bash
set -euo pipefail

BASE="${ORDERS_URL:-http://localhost:8092}"

echo "A20 batch fan-in: publish 8 batch messages rapidly"
pids=()
for i in 1 2 3 4 5 6 7 8; do
  curl --max-time 10 -sS -X POST "$BASE/order?batch=1" \
    -o /dev/null -w "batch-$i [%{http_code}]\n" &
  pids+=("$!")
done

for pid in "${pids[@]}"; do
  wait "$pid"
done

sleep 1
echo "Check in Parallax: one consume_batch span has messaging.batch.message_count=8 and 8 span links to producer traces."
