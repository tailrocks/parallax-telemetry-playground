#!/usr/bin/env bash
# A12: an invocation-scoped CLI run. Under the Parallax fan-out lab, run via:
#   source <parallax>/bench/otlp-fanout/lab.env
#   parallax invocation start -- scenarios/a12-cli-run.sh
# which stamps cli.invocation.id and forwards telemetry to Rotel.
set -euo pipefail
exec "$(dirname "$0")/../target/debug/playground"
