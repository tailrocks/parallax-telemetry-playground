#!/usr/bin/env bash
# c10: a18 canary must not appear on bundle, MCP, webhook, Sentry ack, or UI GraphQL.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health
c_require_bin
c_require_mcp

mkdir -p "$C_ISOLATION"
hook_dir="$(c_isolation_dir)"
python3 - "$hook_dir" <<'PY' &
import http.server, socketserver, sys, pathlib
d = pathlib.Path(sys.argv[1])
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n)
        (d / "body.bin").write_bytes(body)
        self.send_response(200); self.end_headers(); self.wfile.write(b"ok")
    def log_message(self, *args):
        pass
httpd = socketserver.TCPServer(("127.0.0.1", 0), H)
(d / "port").write_text(str(httpd.server_address[1]))
httpd.handle_request()
PY
hook_pid=$!
for _ in $(seq 1 20); do
  [[ -f "$hook_dir/port" ]] && break
  sleep 0.05
done
hook_port="$(cat "$hook_dir/port")"

dest="$(c_gql "mutation { alertDestinationSave(name: \"c10-hook\", kind: \"webhook\", config: \"{\\\"url\\\":\\\"http://127.0.0.1:${hook_port}/c10\\\"}\") { id } }")"
dest_id="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['alertDestinationSave']['id'])" "$dest")"
echo "c10 dest=$dest_id hook=$hook_port"

# Plant the real a18 canary via checkout.
curl -sS -o /dev/null -w "canary http %{http_code}\n" \
  "${CHECKOUT_URL:-http://127.0.0.1:8088}/checkout?canary=1" || true

# Wait for an issue we can bundle.
fp=""
for _ in $(seq 1 20); do
  data="$(c_gql '{ issues(limit: 8) { items { fingerprint title } } }')"
  fp="$(python3 -c "import json,sys; items=((json.loads(sys.argv[1]) or {}).get('issues') or {}).get('items') or []; print(items[0]['fingerprint'] if items else '')" "$data")"
  if [[ -n "$fp" ]]; then break; fi
  sleep 0.5
done
[[ -n "$fp" ]] || { echo "c10: no fingerprint" >&2; exit 1; }

bundle="$(c_gql "{ bundle(fingerprint: \"$fp\") { markdown json } }")"
printf '%s' "$bundle" | c_assert_no_canary "bundle"

cli_ctx="$("$PARALLAX_BIN" issue context "$fp" --format json)"
printf '%s' "$cli_ctx" | c_assert_no_canary "cli-issue-context"

mcp_out="$("$PARALLAX_MCP" --url "$PARALLAX_URL" check --fingerprint "$fp" --parallax-bin "$PARALLAX_BIN" || true)"
printf '%s' "$mcp_out" | c_assert_no_canary "mcp-check"

# UI GraphQL issue + logs (what the SPA reads).
ui="$(c_gql "{ issue(fingerprint: \"$fp\") { title errorType culprit latestEvent { message } } logs(limit: 20, query: \"canary\") { body service } }")"
printf '%s' "$ui" | c_assert_no_canary "ui-graphql"

# Sentry ack must not echo canary.
event="{\"event_id\":\"c10c10c10c10c10c10c10c10c10c10c1\",\"timestamp\":\"2026-08-14T12:00:00Z\",\"platform\":\"python\",\"message\":\"c10 sentry ack\",\"extra\":{\"email\":\"$C_CANARY_EMAIL\"}}"
header="{\"event_id\":\"c10c10c10c10c10c10c10c10c10c10c1\",\"sent_at\":\"2026-08-14T12:00:00Z\"}"
item="{\"type\":\"event\",\"length\":${#event}}"
envelope="$(printf '%s\n%s\n%s' "$header" "$item" "$event")"
ack="$(curl -sS -D - -o /dev/null -X POST "$PARALLAX_URL/api/1/envelope/" \
  -H "content-type: application/x-sentry-envelope" \
  -H "X-Sentry-Auth: Sentry sentry_key=c8public, sentry_version=7" \
  --data-binary "$envelope" || true)"
printf '%s' "$ack" | c_assert_no_canary "sentry-ack"

# Webhook body if anything arrived (best-effort; dest is registered).
if [[ -f "$hook_dir/body.bin" ]]; then
  c_assert_no_canary "webhook" <"$hook_dir/body.bin"
  echo "c10 webhook body checked"
else
  echo "c10 webhook: no delivery yet (dest registered; no leak observed)"
fi

kill "$hook_pid" 2>/dev/null || true
wait "$hook_pid" 2>/dev/null || true
echo "c10 ok fingerprint=$fp dest=$dest_id"
