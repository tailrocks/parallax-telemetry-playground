#!/usr/bin/env bash
# B15: current browser order and analytics journeys.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/web"
exec bun x playwright test e2e/journey.spec.ts --grep 'orders reads durable status|analytics reads ClickHouse' "$@"
