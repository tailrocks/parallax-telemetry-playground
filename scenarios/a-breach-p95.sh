#!/usr/bin/env bash
# a-breach-p95 (plan 167): sustained slow handler — recommendation serves
# every request with injected latency for >=3 minutes so a p95 latency rule
# breaches. Uses the existing deterministic ?slow= knob (b13 mechanics).
set -euo pipefail

BASE="${RECOMMENDATION_URL:-http://localhost:8091}"
BREACH_SECONDS="${BREACH_SECONDS:-200}"
REQUEST_GAP_SECONDS="${REQUEST_GAP_SECONDS:-2}"
SLOW_MS="${BREACH_SLOW_MS:-900}"

echo "a-breach-p95: driving ${SLOW_MS}ms recommendation responses for ${BREACH_SECONDS}s"
end=$((SECONDS + BREACH_SECONDS))
count=0
while ((SECONDS < end)); do
  curl --max-time 30 -sS "$BASE/recommend?sku=WIDGET-1&slow=$SLOW_MS" -o /dev/null \
    -w "slow request [%{http_code}] %{time_total}s\n" || true
  count=$((count + 1))
  echo "a-breach-p95: request #$count ($((end - SECONDS))s remaining)"
  sleep "$REQUEST_GAP_SECONDS"
done

echo "a-breach-p95: done."
echo "Check in Parallax UI: Alerts — a p95 latency rule scoped to recommendation opens an incident."
