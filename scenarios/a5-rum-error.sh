#!/usr/bin/env bash
# A5: browser journey that deliberately records the RUM error path.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/web"
exec bun x playwright test e2e/journey.spec.ts --grep 'forced RUM error' "$@"
