#!/usr/bin/env bash
# A23: Rust Juniper GraphQL resolver -> Java payment gRPC gateway.
set -euo pipefail

BASE="${STOREFRONT_URL:-http://localhost:8094}"
curl --fail-with-body --max-time 20 "$BASE/graphql" \
  -H 'content-type: application/json' \
  --data '{"operationName":"StorefrontQuote","query":"query StorefrontQuote { quote(sku: \"WIDGET-1\", quantity: 2) { sku quantity totalMinor currency } }"}'
