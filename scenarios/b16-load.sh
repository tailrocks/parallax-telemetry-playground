#!/usr/bin/env bash
# B16: k6 checkout load entry point.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v k6 >/dev/null || { echo "B16 requires k6 on PATH" >&2; exit 127; }
exec k6 run "$ROOT/loadgen/checkout.js" "$@"
