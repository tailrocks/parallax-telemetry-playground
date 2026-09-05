#!/usr/bin/env bash
# A26: Catalog-backed recommendation reads plus bounded stampede/slow/leak
# chaos. Normal recommendations have no process-local cache.
set -euo pipefail

BASE="${RECOMMENDATION_URL:-http://localhost:8090}"
SKU="${SKU:-WIDGET-1}"

request() {
  local url="$1"
  curl -fsS "$url"
}

echo "catalog phase: 10 same-SKU requests"
for i in $(seq 1 10); do
  body="$(request "$BASE/recommend?tenant_id=tenant-acme&sku=$SKU&limit=8")"
  printf '%s\n' "  same-${i} ${body:0:500}"
done

echo "catalog phase: multiple seeded SKUs"
for sku in WIDGET-1 WIDGET-2 GADGET-1 GADGET-2; do
  request "$BASE/recommend?tenant_id=tenant-acme&sku=$sku&limit=8" >/dev/null
done
echo "multiple-SKU phase done"

echo "stampede phase: 10 bounded parallel Catalog requests"
request "$BASE/recommend?tenant_id=tenant-acme&sku=$SKU&stampede=10"
echo

echo "A26 done."
echo "Check in Parallax: recommendation spans show Catalog GraphQL fan-out; chaos attributes show stampede_workers, and normal results contain source=catalog-graphql."
