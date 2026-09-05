#!/usr/bin/env bash
# A5: current browser catalog-to-checkout journey.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/web"
export PLAYGROUND_COMPOSE_E2E=1
if [[ -n "${WEB_URL:-}" ]]; then
  export PLAYGROUND_COMPOSE_BASE_URL="$WEB_URL"
fi
exec bun x playwright test e2e/compose.smoke.spec.ts --grep 'browses, quotes, checks out' "$@"
