#!/usr/bin/env bash
# A22: bounded async checkout-delay pressure. Start the compose stack first.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
DELAY_MS="${DELAY_MS:-8000}"
CONCURRENCY="${CONCURRENCY:-12}"

echo "baseline checkout:"
curl -fsS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data '{"tenant_id":"tenant-acme","customer_id":"customer-acme-ava","items":[{"sku":"WIDGET-1","quantity":1}],"currency_code":"USD","payment_method_token":"tok_visa","payment_method_type":"card","request_id":"a22-baseline"}' \
  -o /dev/null -w "  %{time_total}s [%{http_code}]\n"

echo "flooding delayed checkout: delay_ms=$DELAY_MS concurrency=$CONCURRENCY"
flood_log="$(mktemp)"
trap 'rm -f "$flood_log"' EXIT
curl -sS -X POST "$BASE/checkout" \
  -H 'content-type: application/json' \
  --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"delay_ms\":$DELAY_MS,\"request_id\":\"a22-delay\"}" \
  -o /dev/null -w "  delayed request %{time_total}s [%{http_code}]\n" >"$flood_log" &
flood_pid="$!"
sleep 1

echo "concurrent checkout traffic:"
checkout_pids=()
for i in $(seq 1 "$CONCURRENCY"); do
  curl -sS -X POST "$BASE/checkout" \
    -H 'content-type: application/json' \
    --data "{\"tenant_id\":\"tenant-acme\",\"customer_id\":\"customer-acme-ava\",\"items\":[{\"sku\":\"WIDGET-1\",\"quantity\":1}],\"currency_code\":\"USD\",\"payment_method_token\":\"tok_visa\",\"payment_method_type\":\"card\",\"request_id\":\"a22-concurrent-$i\"}" \
    -o /dev/null -w "  checkout-$i %{time_total}s [%{http_code}]\n" &
  checkout_pids+=("$!")
done
for pid in "${checkout_pids[@]}"; do
  wait "$pid"
done
wait "$flood_pid"
cat "$flood_log"

echo "A22 done. Check in Parallax: concurrent checkout spans and bounded delay/in-flight request pressure in the same window."
