#!/usr/bin/env bash
# B4 cascading/partial degradation, B12 release regression (RELEASE=v2),
# B13 slow recommendation, B18 real backdated child span, B16 = loadgen/checkout.js (k6).
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
echo "B4 degrade:"; curl -sS "$BASE/checkout?fail=1&degrade=1" -o /dev/null -w " [%{http_code}]\n"
echo "B18 skew:";    curl -sS "$BASE/checkout?skew=1" -o /dev/null -w " [%{http_code}]\n"
echo "B12: run checkout with RELEASE=v2 to make it regress."
