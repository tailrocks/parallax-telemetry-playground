#!/usr/bin/env bash
# B2: deterministic inventory reservation failure.
set -euo pipefail

BASE="${INVENTORY_URL:-http://localhost:8089}"
code="$(curl --max-time 20 -sS "$BASE/reserve?sku=WIDGET-1&quantity=1&fail=1" -o /dev/null -w '%{http_code}')"
[[ "$code" == "503" ]] || { echo "expected inventory 503, got $code" >&2; exit 1; }
echo "B2 done — inspect inventory error span and downstream checkout impact."
