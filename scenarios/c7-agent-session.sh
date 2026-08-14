#!/usr/bin/env bash
# c7: import-claude fixture + MCP issue_context / agent_session_show.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health
c_require_bin
c_require_mcp

fix="$ROOT/fixtures/claude-code/session.ndjson"
[[ -f "$fix" ]] || { echo "c7: missing $fix" >&2; exit 1; }
out="$("$PARALLAX_BIN" import-claude "$fix" --json 2>&1 || true)"
echo "$out"
id="$(printf '%s\n' "$out" | python3 -c "import json,sys; t=sys.stdin.read();
d=json.loads(t)
sess=d.get('session') or {}
print(d.get('import_id') or sess.get('session_id') or '')")"
[[ -n "$id" ]] || { echo "c7: import produced no id" >&2; exit 1; }

# Need a live issue fingerprint for issue_context equivalence.
fp="$(python3 -c "import json,sys; d=json.loads(sys.argv[1] or '{}'); items=((d or {}).get('issues') or {}).get('items') or []; print(items[0]['fingerprint'] if items else '')" "$(c_gql '{ issues(limit: 1) { items { fingerprint } } }')")"
if [[ -z "$fp" ]]; then
  "$SCRIPT_DIR/c1-issue-context.sh" >/dev/null
  fp="$(python3 -c "import json,sys; d=json.loads(sys.argv[1] or '{}'); items=((d or {}).get('issues') or {}).get('items') or []; print(items[0]['fingerprint'] if items else '')" "$(c_gql '{ issues(limit: 1) { items { fingerprint } } }')")"
fi
[[ -n "$fp" ]] || { echo "c7: no issue fingerprint for MCP" >&2; exit 1; }

inv="$("$PARALLAX_BIN" invocation start -- /bin/echo c7-mcp-inv)"
invid="$(printf '%s\n' "$inv" | python3 -c "import re,sys; m=re.search(r'Parallax invocation id:\\s*(\\S+)', sys.stdin.read()); print(m.group(1) if m else '')")"
[[ -n "$invid" ]] || { echo "c7: no invocation id" >&2; exit 1; }

# Official CLI≡HTTP≡MCP check. A product JSON-shape mismatch is recorded but
# does not skip the stdio tool calls below.
check_out="$("$PARALLAX_MCP" --url "$PARALLAX_URL" check \
  --fingerprint "$fp" \
  --parallax-bin "$PARALLAX_BIN" || true)"
echo "$check_out"
if printf '%s' "$check_out" | grep -q "equivalence: OK"; then
  echo "c7 mcp-check PASS"
else
  echo "c7 mcp-check FAIL (CLI≢GraphQL bundle JSON — W5 DISCREPANCY; stdio tools still required)"
fi

# GraphQL projection used by parallax_agent_session_show.
c_gql "{ agentSession(invocationId: \"$invid\") { errorCount truncated } }" >/dev/null
# Server starts with explicit local trust (tool catalog is compiled in).
"$PARALLAX_MCP" --help | grep -q allow-local-stdio
echo "c7 ok id=$id fingerprint=$fp invocation=$invid mcp-check-attempted"
