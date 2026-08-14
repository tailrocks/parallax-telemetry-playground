#!/usr/bin/env bash
# c2: wrap a short command in `parallax invocation start` and assert list/inspect/bundle.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health
c_require_bin

out="$("$PARALLAX_BIN" invocation start -- /bin/echo c2-ok)"
echo "$out"
id="$(printf '%s\n' "$out" | python3 -c "import re,sys; t=sys.stdin.read(); m=re.search(r'Parallax invocation id:\\s*(\\S+)', t); print(m.group(1) if m else '')")"
if [[ -z "$id" ]]; then
  listed="$("$PARALLAX_BIN" invocation list --format json 2>/dev/null || true)"
  id="$(printf '%s' "$listed" | python3 -c "import json,sys,re; t=sys.stdin.read();
try:
 d=json.loads(t)
except Exception:
 print(''); raise SystemExit
rows=d if isinstance(d,list) else d.get('items') or d.get('invocations') or []
print(rows[0].get('id') or rows[0].get('invocationId') or '' if rows else '')")"
fi
[[ -n "$id" ]] || { echo "c2: could not parse invocation id" >&2; exit 1; }
"$PARALLAX_BIN" invocation inspect "$id" >/dev/null
"$PARALLAX_BIN" invocation bundle "$id" --format json >/dev/null
echo "c2 ok invocation=$id"
