#!/usr/bin/env bash
# A2: generate catalog traffic while the JVM agent's trace-based exemplar filter is enabled.
set -euo pipefail

BASE="${CATALOG_URL:-http://localhost:8090}"
REQUESTS="${A2_REQUESTS:-12}"
query='{"query":"query Exemplars { products { id sku name } }"}'

for i in $(seq 1 "$REQUESTS"); do
  curl --fail-with-body --max-time 20 -sS "$BASE/graphql" \
    -H 'content-type: application/json' -d "$query" >/dev/null
  printf 'catalog query %s/%s\n' "$i" "$REQUESTS"
done

echo "A2 done — inspect catalog.product.queries exemplars and linked trace IDs in each backend."
