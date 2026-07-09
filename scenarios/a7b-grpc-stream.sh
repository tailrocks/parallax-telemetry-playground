#!/usr/bin/env bash
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"

request() {
  local label="$1"
  local query="$2"
  local body
  body="$(curl --max-time 15 -sS "$BASE/quote-stream$query")"
  printf "%-18s %s\n" "$label" "$body"
}

echo "A7b gRPC stream events"
request "clean stream" "?sku=WIDGET-1&quantity=6"
request "fail at 4" "?sku=WIDGET-1&quantity=6&fail_at=4"
request "cancel at 180ms" "?sku=WIDGET-1&quantity=10&cancel_ms=180"

echo "Check in Parallax: pricing stream span has SENT events, checkout span has RECEIVED events; fail_at run records stream_failed; cancel run records server-side cancellation."
