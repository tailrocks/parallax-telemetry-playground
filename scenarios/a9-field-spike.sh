#!/usr/bin/env bash
# A9: current CLI log-pattern corpus, including its late shape.template=spike.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$ROOT/scenarios/corner-cases.sh" l-patterns "$@"
