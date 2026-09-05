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

SHAPES_IDS=(t-deep t-wide t-multiroot t-orphan t-skew t-zero t-links t-longnames t-events l-burst l-bodies l-patterns m-shapes m-labels f-attrs eco-external e-burst e-multi-lang)
JOURNEY_IDS=(j-happy j-error j-outside j-reattach j-parallel)
PROTOCOL_IDS=(p-grpc-err p-grpc-stream p-graphql-err p-rabbitmq-lag)
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
    # NOTE: no bare `e-*` glob here — it would also match eco-full and route
    # it to `shapes eco-full`, which does not exist.
    t-*|l-*|m-*|f-*|e-burst|e-multi-lang|eco-external)
      require_binary
      "$BIN" shapes "$id"
      ;;
    j-happy)
      require_binary
      "$BIN" console --seconds 6
      ;;
    j-error)
      require_binary
      # The CLI process reports the simulated action failure in telemetry while
      # still exiting cleanly; assert the intended event rather than accepting
      # any process result.
      local output output_status
      if output="$("$BIN" console --seconds 6 --fail-at checkout.submit 2>&1)"; then
        output_status=0
      else
        output_status=$?
      fi
      [[ "$output_status" == "1" ]] || {
        echo "forced checkout.submit run returned process status $output_status" >&2
        return 1
      }
      grep -q 'ui.action.name=checkout.submit' <<<"$output" || {
        echo "forced checkout.submit action was not emitted" >&2
        return 1
      }
      grep -q 'outcome="error"' <<<"$output" || {
        echo "forced checkout.submit action did not fail" >&2
        return 1
      }
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
      "$BIN" daemon
      wait "$first" "$second" "$third"
      ;;
    p-grpc-err)
      # Successful pricing leg, HTTP validation, and pricing deadline over the
      # real checkout adapter (deadline via the existing b3b path).
      "$ROOT/scenarios/a1-checkout.sh"
      local response response_body response_status
      response="$(curl -sS -X POST "http://localhost:8088/checkout" \
        -H 'content-type: application/json' \
        --data '{"tenant_id":"tenant-acme","customer_id":"customer-acme-ava","currency_code":"USD","payment_method_token":"tok_visa","payment_method_type":"card","items":[{"sku":"","quantity":0}]}' \
        -w '\n%{http_code}')" || {
        echo "invalid checkout request could not be sent" >&2
        return 1
      }
      response_status="${response##*$'\n'}"
      response_body="${response%$'\n'*}"
      [[ "$response_status" == "400" ]] || {
        echo "invalid checkout request returned HTTP $response_status" >&2
        return 1
      }
      grep -q '"error"' <<<"$response_body" || {
        echo "invalid checkout request returned no typed error" >&2
        return 1
      }
      "$ROOT/scenarios/b3b-grpc-deadline.sh"
      ;;
    p-grpc-stream)
      "$ROOT/scenarios/a7b-grpc-stream.sh"
      ;;
    p-graphql-err)
      "$ROOT/scenarios/a6-graphql.sh"
      ;;
    p-rabbitmq-lag)
      "$ROOT/scenarios/b-async-chaos.sh"
      "$ROOT/scenarios/a4-reverse.sh"
      ;;
    eco-full)
      require_binary
      # One pass across every ecosystem edge: browser (RUM journey),
      # storefront → catalog/pricing, fulfillment RabbitMQ leg, and CLI →
      # checkout, so cli/browser/service node kinds all appear.
      "$ROOT/scenarios/a1-checkout.sh"
      "$ROOT/scenarios/a23-storefront-grpc.sh"
      "$ROOT/scenarios/a24-storefront-catalog.sh"
      "$ROOT/scenarios/a4-reverse.sh"
      "$ROOT/scenarios/a28-rum-journey.sh"
      "$BIN"
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
