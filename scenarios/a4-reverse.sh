#!/usr/bin/env bash
# A4: cross-language async reverse hop. Java `fulfillment` produces to the real
# Kafka `orders` topic, consumes it (CONSUMER span + link), then calls Rust
# `notifications` over HTTP — one trace spanning Java → Kafka → Java → Rust.
# Compare in each backend: does the producer→consumer span link render, and does
# the Java→Rust hop stitch into a single trace?
set -euo pipefail
BASE="${FULFILLMENT_URL:-http://localhost:8093}"
for i in $(seq 1 8); do curl -fsS -X POST "$BASE/publish?order=order-$i" -w " [%{http_code}]\n"; done
echo "A4 done — fulfillment(Java) → Kafka → fulfillment(Java) → notifications(Rust)."
