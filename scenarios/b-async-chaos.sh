#!/usr/bin/env bash
# B7 consumer lag, B8 poison message (redelivery → dead-letter).
set -euo pipefail
BASE="${ORDERS_URL:-http://localhost:8092}"
echo "B7 lag:";    curl -sS -X POST "$BASE/order?lag_ms=300" -w " [%{http_code}]\n"
echo "B8 poison:"; curl -sS -X POST "$BASE/order?poison=1" -w " [%{http_code}]\n"
