#!/usr/bin/env bash
# A24: Rust Juniper GraphQL resolver -> Java catalog GraphQL gateway.
set -euo pipefail

BASE="${STOREFRONT_URL:-http://localhost:8094}"
curl --fail-with-body --max-time 20 "$BASE/graphql" \
  -H 'content-type: application/json' \
  --data '{"operationName":"StorefrontCatalog","query":"query StorefrontCatalog { products(tenantId: \"tenant-acme\", page: 0, size: 10, segment: \"standard\") { items { sku name priceMinor category { slug name } variants { sku name price { amountMinor currency } } reviews { stars title } } page size totalElements totalPages hasNext experience } categories(tenantId: \"tenant-acme\") { slug name } }"}'
