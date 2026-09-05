#!/usr/bin/env bash
# A6: GraphQL field spans, batched-vs-N+1 resolver shape, partial errors,
# and operation-name cardinality policy.
set -euo pipefail

BASE="${CATALOG_URL:-http://localhost:8080}"
GRAPHQL="$BASE/graphql"
TENANT_ID="tenant-acme"

post() {
  local label="$1"
  local payload="$2"
  local validation="${3:-success}"
  local expected_sku="${4:-}"
  local outfile
  outfile="$(mktemp)"
  local code
  code="$(curl -sS -o "$outfile" -w "%{http_code}" \
    -H 'content-type: application/json' \
    -H "x-tenant-id: $TENANT_ID" \
    --data "$payload" \
    "$GRAPHQL")"
  echo "[$label] HTTP $code"
  if [[ "$code" != "200" ]]; then
    cat "$outfile" >&2
    rm -f "$outfile"
    return 1
  fi
  if [[ "$validation" == success ]]; then
    if ! jq -e '
      ((.errors // []) | length == 0)
      and ((.data.products.items // []) | type == "array")
      and (((.data.products.items // []) | length) > 0)
    ' "$outfile" >/dev/null; then
      echo "[$label] HTTP 200 response had GraphQL errors or no product data" >&2
      cat "$outfile" >&2
      rm -f "$outfile"
      return 1
    fi
  elif [[ "$validation" == partial ]]; then
    if ! jq -e --arg expected_sku "$expected_sku" '
      ((.errors // []) | length > 0)
      and .data.product.sku == $expected_sku
    ' "$outfile" >/dev/null; then
      echo "[$label] response was not the expected partial product error" >&2
      cat "$outfile" >&2
      rm -f "$outfile"
      return 1
    fi
  else
    echo "[$label] unsupported GraphQL response validation: $validation" >&2
    rm -f "$outfile"
    return 1
  fi
  cat "$outfile"
  echo
  rm -f "$outfile"
}

batched='{"query":"query batchedReviews { products { items { id sku name reviews { text stars } } } }"}'
n_plus_one='{"query":"query slowReviews { products { items { id sku name reviewsSlow { text stars } } } }"}'

op_name="lookup_${RANDOM}_$$"
lookup_payload="{\"query\":\"query ${op_name} { products { items { id } } }\"}"

echo "GraphQL endpoint: $GRAPHQL"
post "batched reviews" "$batched" >/dev/null
post "N+1 reviewsSlow" "$n_plus_one" >/dev/null
synthetic_sku="${CATALOG_SYNTHETIC_RISK_SCORE_FAILURE_SKU:-}"
if [[ -n "$synthetic_sku" ]]; then
  if [[ ! "$synthetic_sku" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "CATALOG_SYNTHETIC_RISK_SCORE_FAILURE_SKU contains unsupported characters" >&2
    exit 1
  fi
  partial="{\"query\":\"query partialRisk(\$sku: String!) { product(sku: \$sku) { id sku name riskScore } }\",\"variables\":{\"sku\":\"$synthetic_sku\"}}"
  partial_response="$(post "partial riskScore ($synthetic_sku synthetic failure)" "$partial" partial "$synthetic_sku")"
  echo "$partial_response"
else
  normal='{"query":"query normalRisk { products { items { id sku name riskScore } } }"}'
  post "normal riskScore (no synthetic failure configured)" "$normal" >/dev/null
fi
post "high-cardinality operation name $op_name" "$lookup_payload" >/dev/null

cat <<CHECKS

A6 done. Check in Parallax:
- Traces -> newest catalog trace for batchedReviews:
  products.items { reviews { ... } } shows one batched reviews/DataLoader fetch span.
- Traces -> newest catalog trace for slowReviews:
  products.items { reviewsSlow { ... } } shows one Product.reviewsSlow fetch span per product.
- Traces -> newest catalog trace for partialRisk:
  When CATALOG_SYNTHETIC_RISK_SCORE_FAILURE_SKU is set, HTTP is 200, response has errors[], and the riskScore field span/event/status marks that explicit synthetic failure.
- Traces -> newest catalog trace for ${op_name}:
  server span name should stay low-cardinality (query / GraphQL query), not include ${op_name}.
CHECKS
