#!/usr/bin/env bash
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
BODY="$(mktemp "${TMPDIR:-/tmp}/b3b-grpc-deadline.XXXXXX")"
trap 'rm -f "$BODY"' EXIT

echo "B3b gRPC deadline: pricing delay exceeds grpc-timeout"
code="$(curl --max-time 15 -sS \
  "$BASE/checkout?sku=WIDGET-1&quantity=1&retry=2&timeout_ms=100&delay_ms=350" \
  -o "$BODY" -w "%{http_code}" || true)"
printf "deadline [%s]\n" "$code"
cat "$BODY"
echo

if [[ "$code" != "502" ]]; then
  echo "expected checkout to return 502 after pricing deadline, got $code" >&2
  exit 1
fi

echo "Check in Parallax: pricing.attempt sibling spans have rpc.grpc.status_code=4 and deadline_exceeded ERROR status."
