#!/usr/bin/env bash
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"

request() {
  local label="$1"
  local query="$2"
  local expected="$3"
  local body
  local code
  body="$(mktemp "${TMPDIR:-/tmp}/a7b.XXXXXX")"
  code="$(curl --max-time 15 -sS "$BASE/quote-stream$query" -o "$body" -w "%{http_code}" || true)"
  printf "%-18s http %s: %s\n" "$label" "$code" "$(<"$body")"
  rm -f "$body"
  [[ "$code" == "$expected" ]] || {
    echo "$label: expected HTTP $expected, got $code" >&2
    return 1
  }
}

echo "A7b gRPC stream events"
request "clean stream" "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=WIDGET-1&quantity=1" 200
request "unknown product" "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=NO-SUCH-SKU&quantity=1" 404
request "client cancellation" "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=WIDGET-1&quantity=1&delay_ms=1" 200

echo "Check in Parallax: pricing stream span has SENT/RECEIVED events; unknown-product run records the current pricing rejection; delay_ms exercises the current client-cancellation path."
