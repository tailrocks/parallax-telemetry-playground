#!/usr/bin/env bash
# A2: generate catalog traffic while the JVM agent's trace-based exemplar filter is enabled.
set -euo pipefail

BASE="${CATALOG_URL:-http://localhost:8080}"
TENANT_ID="tenant-acme"
REQUESTS="${A2_REQUESTS:-12}"
query='{"query":"query Exemplars { products { items { id sku name } } }"}'

for i in $(seq 1 "$REQUESTS"); do
  response="$(curl --fail-with-body --max-time 20 -sS "$BASE/graphql" \
    -H "x-tenant-id: $TENANT_ID" \
    -H 'content-type: application/json' \
    -d "$query")"
  if ! jq -e '
    ((.errors // []) | length == 0)
    and ((.data.products.items // []) | type == "array")
    and (((.data.products.items // []) | length) > 0)
  ' <<<"$response" >/dev/null; then
    echo "catalog exemplar query returned GraphQL errors or no product data" >&2
    echo "$response" >&2
    exit 1
  fi
  printf 'catalog query %s/%s\n' "$i" "$REQUESTS"
done

echo "A2 done — inspect catalog.product.queries exemplars and linked trace IDs in each backend."
