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
import json, os, sys, urllib.request
url, query = sys.argv[1], sys.argv[2]
headers = {"content-type": "application/json"}
token = os.environ.get("PARALLAX_API_TOKEN", "").strip()
if token:
    headers["authorization"] = f"Bearer {token}"
timeout = float(os.environ.get("PARALLAX_GRAPHQL_TIMEOUT_SECONDS", "30"))
if timeout <= 0:
    raise SystemExit("PARALLAX_GRAPHQL_TIMEOUT_SECONDS must be positive")
req = urllib.request.Request(
    url.rstrip("/") + "/graphql",
    data=json.dumps({"query": query}).encode(),
    headers=headers,
)
with urllib.request.urlopen(req, timeout=timeout) as resp:
    body = json.loads(resp.read().decode())
if body.get("errors"):
    raise SystemExit(f"graphql errors: {body['errors']}")
json.dump(body.get("data"), sys.stdout)
PY
}

c_validate_parallax_mode() {
  case "${SCENARIO_PARALLAX_MODE:-required}" in
    required|skip) ;;
    *)
      echo "SCENARIO_PARALLAX_MODE must be required or skip" >&2
      return 1
      ;;
  esac
}

c_parallax_required() {
  [[ "${SCENARIO_PARALLAX_MODE:-required}" == "required" ]]
}

c_new_trace_id() {
  local trace_id
  trace_id="$(od -An -N16 -tx1 /dev/urandom | tr -d '[:space:]')"
  [[ "$trace_id" =~ ^[[:xdigit:]]{32}$ ]] || {
    echo "could not generate a 32-hex trace id" >&2
    return 1
  }
  printf '%s' "${trace_id,,}"
}

c_traceparent_for() {
  local trace_id="$1"
  local span_number="${2:-1}"
  [[ "$trace_id" =~ ^[[:xdigit:]]{32}$ ]] || {
    echo "trace id is not 32 hexadecimal characters" >&2
    return 1
  }
  [[ "$span_number" =~ ^[0-9]+$ && "$span_number" -gt 0 ]] || {
    echo "span number must be a positive integer" >&2
    return 1
  }
  printf '00-%s-%016x-01' "${trace_id,,}" "$span_number"
}

c_wait_for_trace() {
  local trace_id="$1"
  local label="$2"
  local query="$3"
  local filter="$4"
  local timeout_seconds="${PARALLAX_TRACE_TIMEOUT_SECONDS:-30}"
  local poll_seconds="${PARALLAX_TRACE_POLL_SECONDS:-1}"
  local response=""

  [[ "$trace_id" =~ ^[[:xdigit:]]{32}$ ]] || {
    echo "$label: invalid trace id" >&2
    return 1
  }
  [[ "$timeout_seconds" =~ ^[1-9][0-9]*$ ]] || {
    echo "PARALLAX_TRACE_TIMEOUT_SECONDS must be a positive integer" >&2
    return 1
  }
  [[ "$poll_seconds" =~ ^[1-9][0-9]*$ ]] || {
    echo "PARALLAX_TRACE_POLL_SECONDS must be a positive integer" >&2
    return 1
  }

  local deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS <= deadline )); do
    if response="$(c_gql "$query")" && jq -e "$filter" <<<"$response" >/dev/null; then
      echo "Parallax trace PASS: $label"
      return 0
    fi
    (( SECONDS >= deadline )) && break
    sleep "$poll_seconds"
  done

  echo "Parallax trace FAIL: $label (trace=$trace_id)" >&2
  [[ -z "$response" ]] || printf '%s\n' "$response" >&2
  return 1
}

c_require_health() {
  local code
  local curl_args=(-sS --max-time "${PARALLAX_HTTP_TIMEOUT_SECONDS:-10}" -o /dev/null -w "%{http_code}")
  if [[ -n "${PARALLAX_API_TOKEN:-}" ]]; then
    curl_args+=(-H "authorization: Bearer $PARALLAX_API_TOKEN")
  fi
  if code="$(curl "${curl_args[@]}" "$PARALLAX_URL/health")"; then
    :
  else
    code=000
  fi
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
