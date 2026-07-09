#!/usr/bin/env bash
# B23: one detached checkout log emitted without trace/span context.
set -euo pipefail

BASE="${CHECKOUT_BASE:-http://localhost:8088}"

curl -sS "$BASE/checkout?sku=WIDGET-1&quantity=1&rogue_log=1" -o /dev/null -w "rogue log [%{http_code}]\n"
sleep 1

echo "B23 done."
echo "Check in Parallax: Logs has an error row 'orphan diagnostic without trace context' with no trace chip."
