#!/usr/bin/env bash
# A24: Rust Juniper GraphQL resolver -> Java catalog GraphQL gateway.
set -euo pipefail

BASE="${STOREFRONT_URL:-http://localhost:8094}"
curl --fail-with-body --max-time 20 "$BASE/graphql" \
  -H 'content-type: application/json' \
  --data '{"operationName":"StorefrontCatalog","query":"query StorefrontCatalog { catalogProducts { sku name priceMinor relatedSku riskScore } }"}'
