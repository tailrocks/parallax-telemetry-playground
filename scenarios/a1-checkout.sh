#!/usr/bin/env bash
# A1: checkout flow → distributed trace (HTTP checkout → gRPC pricing).
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
for q in 1 2 3 5; do curl -fsS "$BASE/checkout?sku=WIDGET-1&quantity=$q"; echo; done
echo "A1 done — inspect traces in each backend UI."
