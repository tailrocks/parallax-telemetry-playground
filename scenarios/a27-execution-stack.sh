#!/usr/bin/env bash
# A27: host CLI -> daemon -> simulated container -> agent/tool spans.
# Runs a stitched variant and an orphan variant with child context injection
# deliberately omitted.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE_RESOURCE_ATTRIBUTES="${OTEL_RESOURCE_ATTRIBUTES:-deployment.environment.name=playground}"
INVOCATION_ID="${CLI_INVOCATION_ID:-$(uuidgen | tr "[:upper:]" "[:lower:]")}"
ORPHAN_INVOCATION_ID="$(uuidgen | tr "[:upper:]" "[:lower:]")"

resource_attrs() {
  local invocation_id="$1"
  if [[ "$BASE_RESOURCE_ATTRIBUTES" == *"cli.invocation.id="* ]]; then
    printf "%s" "$BASE_RESOURCE_ATTRIBUTES"
  elif [[ -z "$BASE_RESOURCE_ATTRIBUTES" ]]; then
    printf "cli.invocation.id=%s" "$invocation_id"
  else
    printf "%s,cli.invocation.id=%s" "$BASE_RESOURCE_ATTRIBUTES" "$invocation_id"
  fi
}

export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4317}"
export OTEL_EXPORTER_OTLP_PROTOCOL="${OTEL_EXPORTER_OTLP_PROTOCOL:-grpc}"
export PARALLAX_ENV="${PARALLAX_ENV:-playground}"
export RUST_LOG="${RUST_LOG:-info}"

cargo build -p playground-cli >/dev/null

echo "A27 stitched execution stack invocation: $INVOCATION_ID"
CLI_INVOCATION_ID="$INVOCATION_ID" \
  OTEL_RESOURCE_ATTRIBUTES="$(resource_attrs "$INVOCATION_ID")" \
  "$ROOT/target/debug/playground" daemon --session "$INVOCATION_ID"

echo
echo "A27 orphan execution stack invocation: $ORPHAN_INVOCATION_ID"
CLI_INVOCATION_ID="$ORPHAN_INVOCATION_ID" \
  OTEL_RESOURCE_ATTRIBUTES="$(resource_attrs "$ORPHAN_INVOCATION_ID")" \
  "$ROOT/target/debug/playground" daemon --session "$ORPHAN_INVOCATION_ID" --orphan

echo
echo "Check in Parallax UI:"
echo "- CLI Apps: $INVOCATION_ID shows host_cli -> daemon_session -> container_session -> invoke_agent -> execute_tool"
echo "- CLI Apps: $ORPHAN_INVOCATION_ID keeps the invocation id but splits daemon and child traces"
echo "- Trace evidence gaps: orphan child trace reports browser_without_backend"
