#!/usr/bin/env bash
# B17b cron suite: ok, ok, fail, stuck, missed, duplicate.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/debug/playground"

if [[ ! -x "$BIN" ]]; then
  echo "missing $BIN; run cargo build first" >&2
  exit 1
fi

run_wrapped() {
  if command -v parallax >/dev/null 2>&1; then
    parallax run start -- "$@"
  else
    "$@"
  fi
}

run_slot() {
  local slot="$1"
  local mode="$2"
  local id="playground-report-suite-slot-$slot"
  echo "slot $slot: $mode"
  if [[ "$mode" == "missed" ]]; then
    echo "slot $slot skipped: no process, no telemetry"
    sleep 5
    return 0
  fi
  set +e
  if [[ "$mode" == "duplicate" ]]; then
    run_wrapped "$BIN" cron ok --invocation-id "$id"
    local first_code="$?"
    run_wrapped "$BIN" cron ok --invocation-id "$id"
    local second_code="$?"
    local code="$(( first_code > second_code ? first_code : second_code ))"
  else
    run_wrapped "$BIN" cron "$mode" --invocation-id "$id"
    local code="$?"
  fi
  set -e
  echo "slot $slot exit=$code invocation=$id"
  sleep 5
}

run_slot 1 ok
run_slot 2 ok
run_slot 3 fail
run_slot 4 stuck
run_slot 5 missed
run_slot 6 duplicate

echo "B17b done."
echo "Check in Parallax: Runs show exit codes and durations; slot 5 is absent by design."
echo "Check traces/log attrs: slot 6 has two runs/spans sharing cron.invocation.id."
