#!/usr/bin/env bash
# B19: bounded catalog GraphQL workload using the current products resolver.
set -euo pipefail

BASE="${CATALOG_URL:-http://localhost:8080}"
ROUNDS="${ROUNDS:-4}"
QUERY='{"query":"{ products(tenantId: \"tenant-acme\", page: 0, size: 20, segment: \"standard\") { items { sku name priceMinor reviewsSlow { stars } } } }"}'

echo "baseline products query:"
curl -fsS -H 'content-type: application/json' --data "$QUERY" "$BASE/graphql" >/dev/null

for i in $(seq 1 "$ROUNDS"); do
  echo "products workload round $i"
  curl -fsS -H 'content-type: application/json' --data "$QUERY" "$BASE/graphql" -o /dev/null -w "  products %{time_total}s [%{http_code}]\n"
done

echo "B19 done. Check in Parallax: catalog GraphQL resolver spans and request latency in the same window."
