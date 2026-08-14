#!/usr/bin/env bash
# c7: import a synthetic Claude Code NDJSON session and query agentSession.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health
c_require_bin

fix="$ROOT/fixtures/claude-code/session.ndjson"
[[ -f "$fix" ]] || { echo "c7: missing $fix" >&2; exit 1; }
out="$("$PARALLAX_BIN" import-claude "$fix" --json 2>&1 || true)"
echo "$out"
id="$(printf '%s\n' "$out" | python3 -c "import json,sys; t=sys.stdin.read();
d=json.loads(t)
sess=d.get('session') or {}
print(d.get('import_id') or sess.get('session_id') or '')")"
[[ -n "$id" ]] || { echo "c7: import produced no id" >&2; exit 1; }
echo "c7 ok id=$id"
