#!/usr/bin/env bash
# A26: recommendation in-process cache hit/miss, bypass, and stampede demo.
set -euo pipefail

BASE="${RECOMMENDATION_URL:-http://localhost:8090}"
TTL_MS="${TTL_MS:-300000}"
SKU="${SKU:-A26-WIDGET-$$}"

request() {
  local url="$1"
  curl -fsS "$url"
}

hit_count=0
miss_count=0
echo "cold+warm phase: 10 same-sku requests (expect 1 miss, 9 hits)"
for i in $(seq 1 10); do
  body="$(request "$BASE/recommend?sku=$SKU&ttl_ms=$TTL_MS")"
  if [[ "$body" == *'"cache_hit":true'* ]]; then
    hit_count=$((hit_count + 1))
  else
    miss_count=$((miss_count + 1))
  fi
  echo "  same-$i $body"
done
echo "observed same-sku hits=$hit_count misses=$miss_count"

echo "cache bypass baseline:"
request "$BASE/recommend?sku=$SKU&cache=0&ttl_ms=$TTL_MS"
echo

echo "ratio phase: 20 requests across 5 bounded demo SKUs"
for i in $(seq 1 20); do
  idx=$(( (i - 1) % 5 ))
  request "$BASE/recommend?sku=A26-RATIO-$idx-$$&ttl_ms=$TTL_MS" >/dev/null
done
echo "ratio phase done"

echo "stampede phase: invalidate one SKU and spawn 10 unprotected workers"
request "$BASE/recommend?sku=$SKU&ttl_ms=$TTL_MS&stampede=10"
echo

echo "A26 done."
echo "Check in Parallax: Dashboards -> metric cache.hits/cache.misses (rate agg); trace detail -> parallel compute_recommendations spans; Logs/Field explorer -> cache.hit field."
