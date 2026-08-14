#!/usr/bin/env bash
# A30: exercise the teaching up-down counter + bounded-cardinality metric.
# Drives checkout so http.server.active_requests and
# playground.cardinality.events{demo.bucket=0..15} are emitted.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
for sku in WIDGET-1 WIDGET-2 GADGET-1 WIDGET-1; do
  curl -fsS "$BASE/checkout?sku=${sku}&quantity=1" >/dev/null
done
echo "A30 done — Metrics: http.server.active_requests (up-down) and playground.cardinality.events (demo.bucket ≤15)."
