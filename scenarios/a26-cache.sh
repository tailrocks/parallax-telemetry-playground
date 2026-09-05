#!/usr/bin/env bash
# A26: Catalog-backed recommendation reads plus bounded stampede/slow/leak
# chaos. Normal recommendations have no process-local cache.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib-c.sh
source "$SCRIPT_DIR/lib-c.sh"
c_validate_parallax_mode

BASE="${RECOMMENDATION_URL:-http://localhost:8090}"
SKU="${SKU:-WIDGET-1}"
TENANT_ID="${RECOMMENDATION_TENANT_ID:-tenant-acme}"
REPRESENTATIVE_TRACE_ID=""
STAMPEDE_TRACE_ID=""

request() {
  local label="$1"
  local sku="$2"
  local validation="$3"
  local trace_id="$4"
  local body
  local traceparent
  body="$(mktemp "${TMPDIR:-/tmp}/a26.XXXXXX")"
  traceparent="$(c_traceparent_for "$trace_id")"
  if ! curl --fail-with-body --max-time 20 -sS \
    "$BASE/recommend?tenant_id=$TENANT_ID&sku=$sku&limit=8${5:-}" \
    -H "traceparent: $traceparent" -o "$body"; then
    echo "$label: recommendation request failed" >&2
    cat "$body" >&2
    rm -f "$body"
    return 1
  fi

  case "$validation" in
    normal)
      jq -e --arg sku "$sku" --arg tenant "$TENANT_ID" '
        .sku == $sku
        and .tenant_id == $tenant
        and .source == "catalog-graphql"
        and (.product | type == "object")
        and (.products | type == "array")
        and (.variants | type == "array")
        and (.recommended | type == "array")
        and (.chaos.slow_ms == 0)
        and (.chaos.leak_kb == 0)
        and (.chaos.stampede_workers == 0)
      ' "$body" >/dev/null || {
        echo "$label: invalid catalog-backed recommendation response" >&2
        cat "$body" >&2
        rm -f "$body"
        return 1
      }
      ;;
    stampede)
      jq -e --arg sku "$sku" --arg tenant "$TENANT_ID" '
        .sku == $sku
        and .tenant_id == $tenant
        and .source == "catalog-graphql"
        and (.product | type == "object")
        and (.products | type == "array")
        and (.variants | type == "array")
        and (.recommended | type == "array")
        and (.chaos.stampede_workers == 10)
      ' "$body" >/dev/null || {
        echo "$label: bounded stampede was not reflected in the response" >&2
        cat "$body" >&2
        rm -f "$body"
        return 1
      }
      ;;
    *)
      echo "$label: unknown response validation $validation" >&2
      rm -f "$body"
      return 1
      ;;
  esac

  printf '%s: ' "$label"
  cat "$body"
  printf '\n'
  rm -f "$body"
}

if c_parallax_required; then
  c_require_health
fi

echo "catalog phase: 10 same-SKU requests"
for i in $(seq 1 10); do
  trace_id="$(c_new_trace_id)"
  [[ -n "$REPRESENTATIVE_TRACE_ID" ]] || REPRESENTATIVE_TRACE_ID="$trace_id"
  request "same-${i}" "$SKU" normal "$trace_id"
done

echo "catalog phase: multiple seeded SKUs"
for sku in WIDGET-1 WIDGET-2 GADGET-1 GADGET-2; do
  request "sku-${sku}" "$sku" normal "$(c_new_trace_id)" >/dev/null
done
echo "multiple-SKU phase done"

echo "stampede phase: 10 bounded parallel Catalog requests"
STAMPEDE_TRACE_ID="$(c_new_trace_id)"
request "stampede" "$SKU" stampede "$STAMPEDE_TRACE_ID" '&stampede=10'

if c_parallax_required; then
  c_wait_for_trace "$REPRESENTATIVE_TRACE_ID" "A26 normal recommendation -> Catalog GraphQL" \
    "{ trace(traceId: \"$REPRESENTATIVE_TRACE_ID\") { spans { service name } } }" \
    'any(.trace.spans[]?; .service == "recommendation" and .name == "recommend")
     and any(.trace.spans[]?; .service == "recommendation" and .name == "catalog.graphql.recommendations")'
  c_wait_for_trace "$STAMPEDE_TRACE_ID" "A26 bounded Catalog stampede" \
    "{ trace(traceId: \"$STAMPEDE_TRACE_ID\") { spans { service name } } }" \
    'any(.trace.spans[]?; .service == "recommendation" and .name == "recommend")
     and ([.trace.spans[]? | select(.service == "recommendation" and .name == "catalog.graphql.recommendations")] | length >= 10)'
else
  echo "A26 Parallax trace assertions SKIPPED by SCENARIO_PARALLAX_MODE=skip"
fi
