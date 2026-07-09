#!/usr/bin/env bash
# A22: Tokio blocking-pool saturation. Start the compose stack first.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
BLOCK_MS="${BLOCK_MS:-8000}"
BLOCK_N="${BLOCK_N:-768}"
CONCURRENCY="${CONCURRENCY:-12}"

echo "baseline checkout:"
curl -fsS "$BASE/checkout?sku=WIDGET-1&quantity=1" -o /dev/null -w "  %{time_total}s [%{http_code}]\n"

echo "flooding blocking pool: block_ms=$BLOCK_MS block_n=$BLOCK_N"
flood_log="$(mktemp)"
trap 'rm -f "$flood_log"' EXIT
curl -sS "$BASE/checkout?block_ms=$BLOCK_MS&block_n=$BLOCK_N" -o /dev/null -w "  flood trigger %{time_total}s [%{http_code}]\n" >"$flood_log" &
flood_pid="$!"
sleep 1

echo "concurrent checkout traffic:"
checkout_pids=()
for i in $(seq 1 "$CONCURRENCY"); do
  curl -sS "$BASE/checkout?sku=WIDGET-1&quantity=$i" -o /dev/null -w "  checkout-$i %{time_total}s [%{http_code}]\n" &
  checkout_pids+=("$!")
done
for pid in "${checkout_pids[@]}"; do
  wait "$pid"
done
wait "$flood_pid"
cat "$flood_log"

echo "A22 done. Check in Parallax: Services -> checkout -> Runtime lane: tokio.runtime.* gauges; Traces: slow checkout spans in the same window."
