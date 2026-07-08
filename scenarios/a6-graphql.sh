#!/usr/bin/env bash
# A6: GraphQL field spans, batched-vs-N+1 resolver shape, partial errors,
# and operation-name cardinality policy.
set -euo pipefail

BASE="${CATALOG_URL:-http://localhost:8080}"
GRAPHQL="$BASE/graphql"

post() {
  local label="$1"
  local payload="$2"
  local outfile
  outfile="$(mktemp)"
  local code
  code="$(curl -sS -o "$outfile" -w "%{http_code}" \
    -H 'content-type: application/json' \
    --data "$payload" \
    "$GRAPHQL")"
  echo "[$label] HTTP $code"
  if [[ "$code" != "200" ]]; then
    cat "$outfile" >&2
    rm -f "$outfile"
    return 1
  fi
  cat "$outfile"
  echo
  rm -f "$outfile"
}

batched='{"query":"query batchedReviews { products { id sku name reviews { text stars } } }"}'
n_plus_one='{"query":"query slowReviews { products { id sku name reviewsSlow { text stars } } }"}'
partial='{"query":"query partialRisk { products { id sku name riskScore } }"}'
op_name="lookup_${RANDOM}_$$"
lookup_payload="{\"query\":\"query ${op_name} { products { id } }\"}"

echo "GraphQL endpoint: $GRAPHQL"
post "batched reviews" "$batched" >/dev/null
post "N+1 reviewsSlow" "$n_plus_one" >/dev/null
partial_response="$(post "partial riskScore (GADGET-1 errors by design)" "$partial")"
echo "$partial_response"
if ! grep -q '"errors"' <<<"$partial_response"; then
  echo "partial riskScore response did not contain errors[]" >&2
  exit 1
fi
post "high-cardinality operation name $op_name" "$lookup_payload" >/dev/null

cat <<CHECKS

A6 done. Check in Parallax:
- Traces -> newest catalog trace for batchedReviews:
  products { reviews { ... } } shows one batched reviews/DataLoader fetch span.
- Traces -> newest catalog trace for slowReviews:
  products { reviewsSlow { ... } } shows one Product.reviewsSlow fetch span per product.
- Traces -> newest catalog trace for partialRisk:
  HTTP is 200, response has errors[], and the riskScore field span/event/status marks the deterministic GADGET-1 failure.
- Traces -> newest catalog trace for ${op_name}:
  server span name should stay low-cardinality (query / GraphQL query), not include ${op_name}.
CHECKS
