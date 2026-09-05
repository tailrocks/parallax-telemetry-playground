#!/usr/bin/env bash
# A23: Rust Juniper GraphQL resolver -> Pricing gRPC server.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_validate_parallax_mode

BASE="${STOREFRONT_URL:-http://localhost:8094}"
trace_id="$(c_new_trace_id)"
traceparent="$(c_traceparent_for "$trace_id")"
body="$(mktemp "${TMPDIR:-/tmp}/a23.XXXXXX")"
trap 'rm -f "$body"' EXIT

if ! curl --fail-with-body --max-time 20 -sS "$BASE/graphql" -o "$body" \
  -H 'content-type: application/json' \
  -H "traceparent: $traceparent" \
  --data '{"operationName":"StorefrontQuote","query":"query StorefrontQuote($input: QuoteInput!) { quote(input: $input) { quoteId status lines { sku quantity unitPrice { currencyCode amountMinor } lineTotal { currencyCode amountMinor } } subtotal { currencyCode amountMinor } discountTotal { currencyCode amountMinor } taxTotal { currencyCode amountMinor } grandTotal { currencyCode amountMinor } validForSeconds pricingVersion } }","variables":{"input":{"tenantId":"tenant-acme","customerId":"customer-acme-ava","currencyCode":"USD","items":[{"sku":"WIDGET-1","quantity":2}]}}}'; then
  echo "A23: Storefront GraphQL request failed" >&2
  cat "$body" >&2
  exit 1
fi

# HTTP 200 is not success when Juniper returns a GraphQL error envelope.
jq -e '
  ((.errors // []) | length == 0)
  and (.data.quote | type == "object")
  and ((.data.quote.quoteId // "") | (type == "string" and length > 0))
  and (.data.quote.status == "QUOTE_STATUS_READY")
  and ((.data.quote.lines // []) | length == 1)
  and any(.data.quote.lines[]?;
    .sku == "WIDGET-1"
    and .quantity == 2
    and .unitPrice.currencyCode == "USD"
    and .lineTotal.currencyCode == "USD"
  )
  and (.data.quote.subtotal.currencyCode == "USD")
  and (.data.quote.grandTotal.currencyCode == "USD")
  and ((.data.quote.validForSeconds // 0) > 0)
  and ((.data.quote.pricingVersion // "") | (type == "string" and length > 0))
' "$body" >/dev/null || {
  echo "A23: invalid successful quote response or GraphQL errors" >&2
  cat "$body" >&2
  exit 1
}

cat "$body"
printf '\nA23 response assertions PASS\n'

if c_parallax_required; then
  c_require_health
  c_wait_for_trace "$trace_id" "A23 Storefront GraphQL -> Pricing gRPC" \
    "{ trace(traceId: \"$trace_id\") { spans { service name } } }" \
    'any(.trace.spans[]?; .service == "storefront")
     and any(.trace.spans[]?; .service == "pricing" and (.name | test("^pricing\\.quote$")))'
else
  echo "A23 Parallax trace assertion SKIPPED by SCENARIO_PARALLAX_MODE=skip"
fi
