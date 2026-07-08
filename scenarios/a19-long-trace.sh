#!/usr/bin/env bash
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"

echo "A19 long/wide trace"
curl -fsS "$BASE/checkout?sku=A19-STRESS&quantity=1&fan=15&depth=2"
echo
echo "Expected: about 240 synthetic burst spans plus normal checkout fan-out."
echo "Check in Parallax: trace detail stays responsive; waterfall windowing/minimap/lanes consume this shape."
echo "SQL:"
echo "SELECT trace_id, count(*) FROM opentelemetry_traces WHERE span_name LIKE 'burst.l%' GROUP BY trace_id ORDER BY count(*) DESC LIMIT 5;"
