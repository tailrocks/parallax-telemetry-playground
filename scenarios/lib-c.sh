#!/usr/bin/env bash
# Shared helpers for c-series Parallax product-surface scenarios.
# shellcheck shell=bash
set -euo pipefail

PARALLAX_URL="${PARALLAX_URL:-http://127.0.0.1:4000}"
PARALLAX_BIN="${PARALLAX_BIN:-$(command -v parallax || true)}"
if [[ -z "${PARALLAX_BIN}" && -x "${PARALLAX_BIN_FALLBACK:-}" ]]; then
  PARALLAX_BIN="$PARALLAX_BIN_FALLBACK"
fi
if [[ -z "${PARALLAX_BIN}" && -x /Users/donbeave/Projects/tailrocks/parallax-project/parallax/target/debug/parallax ]]; then
  PARALLAX_BIN=/Users/donbeave/Projects/tailrocks/parallax-project/parallax/target/debug/parallax
fi
PARALLAX_MCP="${PARALLAX_MCP:-$(command -v parallax-mcp || true)}"
if [[ -z "${PARALLAX_MCP}" && -x /Users/donbeave/Projects/tailrocks/parallax-project/parallax/target/debug/parallax-mcp ]]; then
  PARALLAX_MCP=/Users/donbeave/Projects/tailrocks/parallax-project/parallax/target/debug/parallax-mcp
fi
C_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Throwaway homes live under the repo, never scratch / never the operator HOME.
C_ISOLATION="${C_ROOT}/.isolation"

# Tokens planted by the A18 Sentry envelope. Must stay off every egress.
C_CANARY_EMAIL="alice@example.com"
C_CANARY_TOKEN="sk-live-CANARY1234567890"
C_CANARY_CARD="4111111111111111"
C_CANARY_JWT="eyJhbGciOiJIUzI1NiJ9.CANARY.sig"

c_gql() {
  local query="$1"
  python3 - "$PARALLAX_URL" "$query" <<'PY'
import json, sys, urllib.request
url, query = sys.argv[1], sys.argv[2]
req = urllib.request.Request(
    url.rstrip("/") + "/graphql",
    data=json.dumps({"query": query}).encode(),
    headers={"content-type": "application/json"},
)
with urllib.request.urlopen(req, timeout=30) as resp:
    body = json.loads(resp.read().decode())
if body.get("errors"):
    raise SystemExit(f"graphql errors: {body['errors']}")
json.dump(body.get("data"), sys.stdout)
PY
}

c_require_health() {
  local code
  code="$(curl -sS -o /dev/null -w "%{http_code}" "$PARALLAX_URL/health" || true)"
  if [[ "$code" != "200" ]]; then
    echo "Parallax health failed at $PARALLAX_URL/health (http $code)" >&2
    exit 1
  fi
}

c_require_bin() {
  if [[ -z "${PARALLAX_BIN:-}" || ! -x "$PARALLAX_BIN" ]]; then
    echo "parallax binary not found; set PARALLAX_BIN" >&2
    exit 1
  fi
}

c_require_mcp() {
  if [[ -z "${PARALLAX_MCP:-}" || ! -x "$PARALLAX_MCP" ]]; then
    echo "parallax-mcp binary not found; set PARALLAX_MCP" >&2
    exit 1
  fi
}

c_isolation_dir() {
  mkdir -p "$C_ISOLATION"
  mktemp -d "${C_ISOLATION}/cXXXXXX"
}

c_assert_no_canary() {
  local label="$1"
  python3 -c "
import sys
label = sys.argv[1]
text = sys.stdin.read()
needles = [
    '${C_CANARY_EMAIL}',
    '${C_CANARY_TOKEN}',
    '${C_CANARY_CARD}',
    '${C_CANARY_JWT}',
]
low = text.lower()
for n in needles:
    if n.lower() in low:
        raise SystemExit(f'{label}: canary leaked ({n[:16]}…)')
" "$label"
}
