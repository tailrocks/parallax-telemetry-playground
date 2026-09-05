#!/usr/bin/env bash
# A5: current browser catalog-to-checkout journey.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/web"
exec bun x playwright test e2e/journey.spec.ts --grep 'catalog to cart to checkout' "$@"
