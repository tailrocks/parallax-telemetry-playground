#!/usr/bin/env bash
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"

echo "A20 structural compare pair"
echo "variant v1"
curl -fsS "$BASE/checkout?sku=A20-COMPARE&quantity=1&variant=v1"
echo
echo "variant v2"
curl -fsS "$BASE/checkout?sku=A20-COMPARE&quantity=1&variant=v2"
echo
echo "Check in Parallax: trace detail -> Compare with... should show added reserve spans and removed recommend span."
echo "Find trace ids:"
echo "SELECT trace_id, \"span_attributes.compare.variant\" AS variant, \"timestamp\" FROM opentelemetry_traces WHERE span_name = 'checkout' AND \"span_attributes.compare.variant\" IS NOT NULL ORDER BY \"timestamp\" DESC LIMIT 2;"
