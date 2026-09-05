#!/usr/bin/env bash
# A23: Rust Juniper GraphQL resolver -> Pricing gRPC server.
set -euo pipefail

BASE="${STOREFRONT_URL:-http://localhost:8094}"
curl --fail-with-body --max-time 20 "$BASE/graphql" \
  -H 'content-type: application/json' \
  --data '{"operationName":"StorefrontQuote","query":"query StorefrontQuote($input: QuoteInput!) { quote(input: $input) { quoteId status lines { sku quantity unitPrice { currencyCode amountMinor } lineTotal { currencyCode amountMinor } } subtotal { currencyCode amountMinor } discountTotal { currencyCode amountMinor } taxTotal { currencyCode amountMinor } grandTotal { currencyCode amountMinor } validForSeconds pricingVersion } }","variables":{"input":{"tenantId":"tenant-acme","customerId":"customer-acme-ava","currencyCode":"USD","items":[{"sku":"WIDGET-1","quantity":2}]}}}'
