#!/usr/bin/env bash
# A19: current CLI wide-trace corpus. The backend emits 521 spans.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec "$ROOT/scenarios/corner-cases.sh" t-wide "$@"
