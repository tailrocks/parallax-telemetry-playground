#!/usr/bin/env bash
# B10: concurrent checkout requests contending for the shared lock.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
REQUESTS="${B10_REQUESTS:-12}"
for i in $(seq 1 "$REQUESTS"); do
  curl --max-time 30 -sS "$BASE/checkout?lock=1&slow=150" -o /dev/null -w "request $i %{time_total}s [%{http_code}]\n" &
done
wait
echo "B10 done — inspect serialized checkout spans and their contention delay."
