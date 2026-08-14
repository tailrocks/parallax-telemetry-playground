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
