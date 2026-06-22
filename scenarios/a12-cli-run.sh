#!/usr/bin/env bash
# A12: a run-scoped CLI invocation. Under the Parallax fan-out lab, run via:
#   source <parallax>/bench/otlp-fanout/lab.env
#   parallax run start -- scenarios/a12-cli-run.sh
# which stamps parallax.run.id and forwards telemetry to Rotel.
set -euo pipefail
exec "$(dirname "$0")/../target/debug/playground"
