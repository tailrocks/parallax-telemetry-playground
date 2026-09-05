#!/usr/bin/env bash
# B16: k6 weighted Storefront browse/product/cart/checkout/order/analytics load.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v k6 >/dev/null || { echo "B16 requires k6 on PATH" >&2; exit 127; }

STOREFRONT_URL="${STOREFRONT_URL:-http://localhost:8094/graphql}"
LOADGEN_RUN_ID="${LOADGEN_RUN_ID:-b16-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
export STOREFRONT_URL LOADGEN_RUN_ID

echo "B16 Storefront load: run=$LOADGEN_RUN_ID url=$STOREFRONT_URL"
exec k6 run "$ROOT/loadgen/checkout.ts" "$@"
