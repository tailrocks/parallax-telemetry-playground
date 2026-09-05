#!/usr/bin/env bash
# A28: real Compose-backed browser journey plus stitched Parallax evidence.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_validate_parallax_mode

WEB_URL="${WEB_URL:-http://localhost:5173}"
E2E_LOG="$(mktemp "${TMPDIR:-/tmp}/a28-e2e.XXXXXX")"
trap 'rm -f "$E2E_LOG"' EXIT

if c_parallax_required; then
  c_require_health
fi

echo "A28 frontend RUM journey"
echo "Smoke-check SSR pages:"
for path in / /catalog /cart /checkout /orders /analytics; do
  curl --fail-with-body --max-time 15 -sS "$WEB_URL$path" -o /dev/null
  printf "  ok %s\n" "$WEB_URL$path"
done

command -v bun >/dev/null 2>&1 || {
  echo "A28: bun is required for the Compose-backed Playwright journey" >&2
  exit 1
}

trace_id="$(c_new_trace_id)"
traceparent="$(c_traceparent_for "$trace_id")"
if c_parallax_required; then
  otlp_endpoint="${PLAYGROUND_TEST_OTLP_ENDPOINT:-${PARALLAX_OTLP_HTTP_TRACES_ENDPOINT:-${PARALLAX_URL%/}/v1/traces}}"
else
  otlp_endpoint="${PLAYGROUND_TEST_OTLP_ENDPOINT:-}"
fi

if ! (
  cd "$ROOT/web"
  export PLAYGROUND_COMPOSE_E2E=1
  export PLAYGROUND_COMPOSE_BASE_URL="$WEB_URL"
  export TRACEPARENT="$traceparent"
  export TRACESTATE="${TRACESTATE:-playground=browser}"
  export BAGGAGE="${BAGGAGE:-tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal}"
  if [[ -n "$otlp_endpoint" ]]; then
    export PLAYGROUND_TEST_OTLP_ENDPOINT="$otlp_endpoint"
  else
    unset PLAYGROUND_TEST_OTLP_ENDPOINT
  fi
  bun run e2e:compose
) >"$E2E_LOG" 2>&1; then
  cat "$E2E_LOG" >&2
  exit 1
fi
cat "$E2E_LOG"
echo "A28 browser journey PASS"

if c_parallax_required; then
  c_wait_for_trace "$trace_id" "A28 browser RUM journey stitching" \
    "{ trace(traceId: \"$trace_id\") { spans { service name attributes resource } } }" \
    'any(.trace.spans[]?; .service == "playground-web-tests" and .name == "test.case")
     and any(.trace.spans[]?; .service == "web")
     and any(.trace.spans[]?; .service == "storefront")
     and any(.trace.spans[]?; .service == "checkout")'
else
  echo "A28 Parallax trace assertion SKIPPED by SCENARIO_PARALLAX_MODE=skip"
fi
