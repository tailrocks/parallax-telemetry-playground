#!/usr/bin/env bash
# B19: bounded JVM heap pressure in catalog while GraphQL products queries run.
set -euo pipefail

BASE="${CATALOG_URL:-http://localhost:8080}"
MB="${MB:-96}"
HOLD_MS="${HOLD_MS:-5000}"
ROUNDS="${ROUNDS:-4}"
QUERY='{"query":"{ products { sku name priceMinor } }"}'

echo "baseline products query:"
curl -fsS -H 'content-type: application/json' --data "$QUERY" "$BASE/graphql" >/dev/null

for i in $(seq 1 "$ROUNDS"); do
  echo "heap pressure round $i: mb=$MB holdMs=$HOLD_MS"
  curl -fsS "$BASE/chaos/heap?mb=$MB&holdMs=$HOLD_MS" >/dev/null &
  sleep 0.3
  curl -fsS -H 'content-type: application/json' --data "$QUERY" "$BASE/graphql" -o /dev/null -w "  products %{time_total}s [%{http_code}]\n"
done
wait

echo "B19 done. Check in Parallax: Services -> catalog -> Runtime lane: jvm.memory.used / jvm.gc.* rise; Traces: slower GraphQL spans in the same window."
