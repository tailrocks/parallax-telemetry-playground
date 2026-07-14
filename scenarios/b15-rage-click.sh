#!/usr/bin/env bash
# B15: browser rage-click journey (the test drives the promo button repeatedly).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/web"
exec bun x playwright test e2e/journey.spec.ts --grep 'rage-click journey' "$@"
