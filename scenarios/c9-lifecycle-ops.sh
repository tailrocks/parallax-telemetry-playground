#!/usr/bin/env bash
# c9: doctor + prune dry-run against the live instance (no --execute on real HOME).
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health
c_require_bin

"$PARALLAX_BIN" doctor
"$PARALLAX_BIN" prune
echo "c9 ok doctor+prune-dry-run"
