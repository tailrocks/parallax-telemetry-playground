#!/usr/bin/env bash
# A31: handled PaymentError (?fail=1 → 502) vs unhandled panic (?unhandled=1 → 500).
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
handled="$(curl -sS -o /tmp/a31-handled.json -w "%{http_code}" "$BASE/checkout?fail=1")"
unhandled="$(curl -sS -o /tmp/a31-unhandled.json -w "%{http_code}" "$BASE/checkout?unhandled=1" || true)"
if [[ "$handled" != "502" ]]; then
  echo "A31: expected handled 502, got $handled" >&2
  exit 1
fi
if [[ "$unhandled" == "502" ]]; then
  echo "A31: unhandled must not look like handled PaymentError" >&2
  exit 1
fi
if [[ "$unhandled" != "500" && "$unhandled" != "000" ]]; then
  echo "A31: expected unhandled 500 (or connection reset), got $unhandled" >&2
  exit 1
fi
echo "A31 done — Issues: handled PaymentError 502 vs unhandled panic $unhandled."
