#!/usr/bin/env bash
set -euo pipefail

BASE="${INVENTORY_URL:-http://localhost:8089}"
TMPDIR="${TMPDIR:-/tmp}"
POOL_BODY="$(mktemp "$TMPDIR/a25-pool.XXXXXX")"
trap 'rm -f "$POOL_BODY"' EXIT

request() {
  local label="$1"
  local url="$2"
  local code
  code="$(curl --max-time 15 -sS "$url" -o /dev/null -w "%{http_code}")"
  printf "%-20s [%s]\n" "$label" "$code"
}

echo "A25 Postgres: normal reserve"
request "normal" "$BASE/reserve?sku=WIDGET-1&quantity=1"
echo "Check in Parallax: reserve trace has UPDATE stock span with db.system.name=postgresql and db.query.text."
echo

echo "A25 Postgres: slow query"
request "pg_sleep 400ms" "$BASE/reserve?sku=WIDGET-2&quantity=1&slow=400"
echo "Check in Parallax: trace has SELECT pg_sleep db span around 400ms."
echo

echo "A25 Postgres: DB N+1"
request "db_n1=12" "$BASE/reserve?sku=WIDGET-3&quantity=1&db_n1=12"
echo "Check in Parallax: trace has 12 SELECT stock spans before UPDATE."
echo

echo "A25 Postgres: pool exhaustion"
pids=()
for i in 1 2 3 4 5 6; do
  curl --max-time 12 -sS "$BASE/reserve?sku=WIDGET-4&quantity=1&hold_ms=4000" \
    -o /dev/null -w "hold-$i [%{http_code}]\n" &
  pids+=("$!")
done

sleep 0.5
pool_code="$(curl --max-time 8 -sS "$BASE/reserve?sku=WIDGET-1&quantity=1" -o "$POOL_BODY" -w "%{http_code}" || true)"
printf "%-20s [%s]\n" "pool pressure" "$pool_code"

for pid in "${pids[@]}"; do
  wait "$pid" || true
done

if [[ "$pool_code" != "503" ]]; then
  echo "expected pool pressure request to return 503, got $pool_code" >&2
  cat "$POOL_BODY" >&2
  exit 1
fi

echo "Check in Parallax: reserve span has pool_exhausted error and db.client.connection.* gauges move."
