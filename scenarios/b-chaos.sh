#!/usr/bin/env bash
# Deliberate-failure catalog (subset) — drive each backend to render the failure.
#   B1 payment failure (502 + error issue), B11 injected latency.
set -euo pipefail
BASE="${CHECKOUT_URL:-http://localhost:8088}"
echo "B1 payment failure:"; curl -sS "$BASE/checkout?fail=1" -w " [%{http_code}]\n"
if [[ "${CROSS_LANGUAGE_PAYMENT_ERROR:-0}" == "1" ]]; then
  echo "Cross-language PaymentError (checkout -> Java payment):"
  curl -sS "$BASE/checkout?sku=PAYMENT-ERROR&quantity=1" -w " [%{http_code}]\n"
  echo "Run with deploy/docker-compose.xlang.yml; compare PaymentError grouping across backends."
fi
echo "B11 latency 500ms:";  curl -sS "$BASE/checkout?slow=500" -o /dev/null -w "  %{time_total}s [%{http_code}]\n"
echo "done — compare the error grouping + slow-span rendering in each backend UI."
