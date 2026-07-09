#!/usr/bin/env bash
# A9: field-spike logs for Field Explorer demos.
set -euo pipefail

BASE="${CHECKOUT_BASE:-http://localhost:8088}"
SCREEN="${SCREEN:-workspace-select}"

for i in 1 2 3; do
  curl -sS "$BASE/checkout?sku=WIDGET-1&quantity=$i" -o /dev/null -w "baseline $i [%{http_code}]\n"
done

curl -sS "$BASE/checkout?sku=WIDGET-1&quantity=1&spike=$SCREEN" -o /dev/null -w "spike $SCREEN [%{http_code}]\n"
sleep 1

echo "A9 done."
echo "Check in Parallax: Logs plus Field Explorer (plan 046)."
echo "In the spike window, app_screen_name should be dominated by $SCREEN."
