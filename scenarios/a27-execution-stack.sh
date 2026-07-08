#!/usr/bin/env bash
# A27: host CLI -> daemon -> simulated container -> agent/tool spans.
# Runs a stitched variant and an orphan variant with child context injection
# deliberately omitted.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE_RESOURCE_ATTRIBUTES="${OTEL_RESOURCE_ATTRIBUTES:-deployment.environment.name=playground}"
RUN_ID="${PARALLAX_RUN_ID:-exec-stack-$(date +%s)-$$}"
ORPHAN_RUN_ID="${RUN_ID}-orphan"

resource_attrs() {
  local run_id="$1"
  if [[ "$BASE_RESOURCE_ATTRIBUTES" == *"parallax.run.id="* ]]; then
    printf "%s" "$BASE_RESOURCE_ATTRIBUTES"
  elif [[ -z "$BASE_RESOURCE_ATTRIBUTES" ]]; then
    printf "parallax.run.id=%s" "$run_id"
  else
    printf "%s,parallax.run.id=%s" "$BASE_RESOURCE_ATTRIBUTES" "$run_id"
  fi
}

export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4317}"
export OTEL_EXPORTER_OTLP_PROTOCOL="${OTEL_EXPORTER_OTLP_PROTOCOL:-grpc}"
export PARALLAX_ENV="${PARALLAX_ENV:-playground}"
export RUST_LOG="${RUST_LOG:-info}"

cargo build -p playground-cli >/dev/null

echo "A27 stitched execution stack run: $RUN_ID"
PARALLAX_RUN_ID="$RUN_ID" \
  OTEL_RESOURCE_ATTRIBUTES="$(resource_attrs "$RUN_ID")" \
  "$ROOT/target/debug/playground" daemon --session "$RUN_ID"

echo
echo "A27 orphan execution stack run: $ORPHAN_RUN_ID"
PARALLAX_RUN_ID="$ORPHAN_RUN_ID" \
  OTEL_RESOURCE_ATTRIBUTES="$(resource_attrs "$ORPHAN_RUN_ID")" \
  "$ROOT/target/debug/playground" daemon --session "$ORPHAN_RUN_ID" --orphan

echo
echo "Check in Parallax UI:"
echo "- Runs/Story: $RUN_ID shows host_cli -> daemon_session -> container_session -> invoke_agent -> execute_tool"
echo "- Runs/Story: $ORPHAN_RUN_ID keeps run.id but splits daemon and child traces"
echo "- Trace evidence gaps: orphan child trace reports browser_without_backend"
