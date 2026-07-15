#!/usr/bin/env bash
# B13: deterministic recommendation slowness.
set -euo pipefail

BASE="${RECOMMENDATION_URL:-http://localhost:8091}"
SLOW_MS="${B13_SLOW_MS:-750}"
curl --fail-with-body --max-time 30 -sS "$BASE/recommend?sku=WIDGET-1&slow=$SLOW_MS" -o /dev/null -w 'recommendation %{time_total}s [%{http_code}]\n'
echo "B13 done — inspect slow recommendation spans and dependent checkout degradation."
