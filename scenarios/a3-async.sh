#!/usr/bin/env bash
# A3: async branch — PRODUCER span (publish) → CONSUMER span (process) with a
# span LINK back to the producer (the messaging causal edge).
set -euo pipefail
BASE="${ORDERS_URL:-http://localhost:8092}"
curl -sS -X POST "$BASE/order" -w " [%{http_code}]\n"
echo "compare: does each backend render the producer→consumer span link?"
