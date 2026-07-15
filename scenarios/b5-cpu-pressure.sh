#!/usr/bin/env bash
# B5: bounded CPU hot path in checkout.
set -euo pipefail

BASE="${CHECKOUT_URL:-http://localhost:8088}"
REQUESTS="${B5_REQUESTS:-12}"
CPU_MS="${B5_CPU_MS:-300}"
for _ in $(seq 1 "$REQUESTS"); do
  curl --max-time 30 -sS "$BASE/checkout?cpu_ms=$CPU_MS" -o /dev/null -w 'cpu request %{time_total}s [%{http_code}]\n'
done

echo "B5 done — inspect checkout CPU/runtime saturation and slow request spans."
