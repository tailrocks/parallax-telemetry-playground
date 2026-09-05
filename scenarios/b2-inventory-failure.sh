#!/usr/bin/env bash
# B2: deterministic inventory reservation failure.
set -euo pipefail

BASE="${INVENTORY_URL:-http://localhost:8089}"
TENANT_ID="${INVENTORY_TENANT_ID:-tenant-acme}"
RUN_ID="${B2_RUN_ID:-b2-$$}"
code="$(curl --max-time 20 -sS "$BASE/reserve?tenant_id=$TENANT_ID&reservation_id=${RUN_ID}-failure&sku=WIDGET-1&quantity=1&fail=1" -o /dev/null -w '%{http_code}')"
[[ "$code" == "503" ]] || { echo "expected inventory 503, got $code" >&2; exit 1; }
echo "B2 done — inspect inventory error span and downstream checkout impact."
