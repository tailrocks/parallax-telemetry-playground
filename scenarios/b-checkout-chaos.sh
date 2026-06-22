#!/usr/bin/env bash
# B3 timeout/retry, B9 N+1 hotspot (extra sequential inventory calls).
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
echo "B3 retry=2 timeout=50ms:"; curl -sS "$BASE/checkout?retry=2&timeout_ms=50" -o /dev/null -w " [%{http_code}]\n"
echo "B9 n+1=8:";                curl -sS "$BASE/checkout?n1=8" -o /dev/null -w " [%{http_code}]\n"
