#!/usr/bin/env bash
# c6: POST GitHub webhook fixtures. Requires [github_deploy]/[github_actions]
# enabled on the target serve (scratch config). Bad HMAC must fail.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_require_health

SECRET="${GITHUB_WEBHOOK_SECRET:-playground-c6-secret}"
payload="$ROOT/fixtures/github/deployment.json"
[[ -f "$payload" ]] || { echo "c6: missing $payload" >&2; exit 1; }

body="$(cat "$payload")"
sig="$(python3 - "$SECRET" "$body" <<'PY'
import hmac, hashlib, sys
secret, body = sys.argv[1], sys.stdin.read() if False else sys.argv[2]
print("sha256=" + hmac.new(secret.encode(), body.encode(), hashlib.sha256).hexdigest())
PY
)"

code="$(curl -sS -o /tmp/c6-ok.txt -w "%{http_code}" -X POST "$PARALLAX_URL/webhooks/github" \
  -H "content-type: application/json" \
  -H "X-GitHub-Event: deployment" \
  -H "X-GitHub-Delivery: 11111111-2222-3333-4444-555555555555" \
  -H "X-Hub-Signature-256: $sig" \
  --data-binary "$body" || true)"
bad="$(curl -sS -o /tmp/c6-bad.txt -w "%{http_code}" -X POST "$PARALLAX_URL/webhooks/github" \
  -H "content-type: application/json" \
  -H "X-GitHub-Event: deployment" \
  -H "X-GitHub-Delivery: 11111111-2222-3333-4444-555555555556" \
  -H "X-Hub-Signature-256: sha256=deadbeef" \
  --data-binary "$body" || true)"

if [[ "$code" != 2* ]]; then
  echo "c6: good HMAC expected 2xx got $code (github ingest may be disabled)" >&2
  exit 1
fi
if [[ "$bad" == 2* ]]; then
  echo "c6: bad HMAC was accepted ($bad)" >&2
  exit 1
fi
echo "c6 ok good=$code bad=$bad"
