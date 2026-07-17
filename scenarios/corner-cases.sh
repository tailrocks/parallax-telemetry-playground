#!/usr/bin/env bash
# Corner-case corpus runner (plan 161): one deterministic scenario per UI
# rendering risk. See docs/corner-case-matrix.md for the scenario→surface→
# expected-rendering contract. Ids are stable API for plans 159/160.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/debug/playground"
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://127.0.0.1:4317}"
export OTEL_EXPORTER_OTLP_PROTOCOL="${OTEL_EXPORTER_OTLP_PROTOCOL:-grpc}"
export PARALLAX_ENV="${PARALLAX_ENV:-playground}"
export RUST_LOG="${RUST_LOG:-info}"

SHAPES_IDS=(t-deep t-wide t-multiroot t-orphan t-skew t-zero t-links t-longnames t-events l-burst l-bodies l-patterns m-shapes m-labels f-attrs e-burst e-multi-lang)
JOURNEY_IDS=(j-happy j-error j-outside j-reattach j-parallel)
PROTOCOL_IDS=(p-grpc-err p-grpc-stream p-graphql-err p-kafka-lag)
ALL_IDS=("${SHAPES_IDS[@]}" "${PROTOCOL_IDS[@]}" "${JOURNEY_IDS[@]}" eco-full)

require_binary() {
  if [[ ! -x "$BIN" ]]; then
    echo "building playground CLI (first run)…"
    (cd "$ROOT" && cargo build -p playground-cli)
  fi
}

run_id() {
  local id="$1"
  echo "── corner case: $id"
  case "$id" in
    t-*|l-*|m-*|f-*|e-*)
      require_binary
      "$BIN" shapes "$id"
      ;;
    j-happy)
      require_binary
      "$BIN" console --seconds 6
      ;;
    j-error)
      require_binary
      # The forced failure exits non-zero by design (outcome=failure).
      "$BIN" console --seconds 6 --fail-at checkout.submit || true
      ;;
    j-outside)
      require_binary
      "$BIN" console --seconds 6 --outside-error
      ;;
    j-reattach)
      require_binary
      "$BIN" console --seconds 9 --reattach 3
      ;;
    j-parallel)
      require_binary
      # Three concurrent invocations plus the daemon sim: four correlation
      # domains interleaving into the same store.
      "$BIN" console --seconds 6 &
      local first=$!
      "$BIN" console --seconds 6 &
      local second=$!
      "$BIN" console --seconds 6 &
      local third=$!
      "$BIN" daemon || true
      wait "$first" "$second" "$third"
      ;;
    p-grpc-err)
      # OK + INVALID_ARGUMENT + DEADLINE_EXCEEDED + UNAVAILABLE over the real
      # pricing gRPC leg (deadline/unavailable via the existing b3b path).
      "$ROOT/scenarios/a1-checkout.sh"
      curl -sf "http://localhost:8088/checkout?sku=&quantity=0" || true
      "$ROOT/scenarios/b3b-grpc-deadline.sh"
      ;;
    p-grpc-stream)
      "$ROOT/scenarios/a7b-grpc-stream.sh"
      ;;
    p-graphql-err)
      "$ROOT/scenarios/a6-graphql.sh"
      ;;
    p-kafka-lag)
      "$ROOT/scenarios/b-async-chaos.sh"
      "$ROOT/scenarios/a4-reverse.sh"
      ;;
    eco-full)
      require_binary
      # One pass across every ecosystem edge: browser (RUM journey),
      # storefront → catalog/payment, fulfillment Kafka leg, and CLI →
      # checkout, so cli/browser/service node kinds all appear.
      "$ROOT/scenarios/a1-checkout.sh"
      "$ROOT/scenarios/a23-storefront-grpc.sh"
      "$ROOT/scenarios/a24-storefront-catalog.sh"
      "$ROOT/scenarios/a4-reverse.sh"
      "$ROOT/scenarios/a28-rum-journey.sh" || true
      "$BIN" || true
      ;;
    *)
      echo "unknown corner-case id: $id" >&2
      return 2
      ;;
  esac
  echo "── corner case: $id done"
}

if [[ "${1:-}" == "--all-corner-cases" ]]; then
  declare -a summary=()
  failures=0
  for id in "${ALL_IDS[@]}"; do
    if run_id "$id"; then
      summary+=("$id  ok")
    else
      summary+=("$id  FAILED")
      failures=$((failures + 1))
    fi
  done
  echo
  echo "corner-case sweep summary:"
  printf '  %s\n' "${summary[@]}"
  exit "$failures"
fi

if [[ $# -lt 1 ]]; then
  echo "usage: corner-cases.sh <id>|--all-corner-cases" >&2
  printf 'ids: %s\n' "${ALL_IDS[*]}" >&2
  exit 2
fi
run_id "$1"
