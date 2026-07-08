#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

catalog() {
  cat <<'TABLE'
ID              Script                         Drives                                      Check in Parallax UI
a1              a1-checkout.sh                 checkout -> pricing/inventory/recommendation Traces: checkout waterfall with downstream children
a6              a6-graphql.sh                  catalog GraphQL field-span family            Traces: batched vs N+1, partial error, op-name policy
a7              a7-subscription.ts             catalog GraphQL subscription smoke            Traces: long-lived priceChanges subscription span
a9              a9-field-spike.sh              checkout structured log burst                Logs/Field Explorer: app_screen_name dominated by workspace-select
a3              a3-async.sh                    orders producer/consumer                    Trace detail: producer span linked to consumer trace
a4              a4-reverse.sh                  fulfillment -> Kafka -> notifications        Trace detail: Java async span link and Java -> Rust hop
a12             a12-cli-run.sh                 playground CLI checkout driver              Runs: command row with exit code; cargo build first
a13             a13-deploy-regression.sh       checkout v1 then v2 recreate                Issues: error spike attributed to service.version=v2
a14             a14-flag-flip.sh               flagd paymentFailure off/on/off              Traces/Issues: feature_flag events and flag-scoped failures
a18             a18-canary.sh                  fake sensitive canary corpus                 Issues/Logs: redaction of fake email/token/card/jwt corpus
a22             a22-tokio-saturation.sh        checkout spawn_blocking flood                Services -> checkout -> Runtime lane: tokio.runtime.* spike; Traces: slow spans
a27             a27-execution-stack.sh         CLI -> daemon -> container -> agent          Runs/Story: stitched beats; orphan child trace has browser_without_backend
b-async-chaos   b-async-chaos.sh               consumer lag and poison message              Services/Traces: lag span and dead-letter error branch
b-chaos         b-chaos.sh                     payment failure and latency                  Issues/Services: checkout error and slow-span rendering
b-checkout-chaos b-checkout-chaos.sh           retry timeout and N+1 fan-out                Traces: retry/timeout branch and N+1 waterfall
b-degradation   b-degradation.sh               partial degrade and skew                     Traces/Issues: degraded response and skewed span timing
b17             b17-cron.sh                    playground cron mode                         Runs: cron success/fail/stuck outcome; cargo build first
b17b            b17b-cron-suite.sh             cron ok/fail/stuck/missed/duplicate          Runs: schedule attrs, missing beat, duplicate invocation id
b19             b19-jvm-gc-pressure.sh         catalog bounded heap pressure                Services -> catalog -> Runtime lane: jvm.memory.used / jvm.gc.*; GraphQL spans slow
b20             b20-container-oom.sh --yes     recommendation leak under 128m overlay       Docker OOM/restart + telemetry gap; destructive, requires --yes
b22             b22-sampling-gap.sh            checkout at 10 percent root sampling         Traces: sampled-out gaps; Logs: full request evidence
b23             b23-uncorrelated-log.sh        detached checkout log outside span context   Logs: error row without trace chip
TABLE
}

scenario() {
  case "$1" in
    a1) echo "a1-checkout.sh|Traces: checkout waterfall with pricing, inventory, and recommendation children" ;;
    a6) echo "a6-graphql.sh|Traces: batched reviews vs reviewsSlow N+1 shape, partial riskScore error, op-name policy" ;;
    a7) echo "a7-subscription.ts|Traces: long-lived priceChanges subscription span; run with Bun" ;;
    a9) echo "a9-field-spike.sh|Logs/Field Explorer: app_screen_name dominated by workspace-select in the spike window" ;;
    a3) echo "a3-async.sh|Trace detail: producer span with link to consumer trace" ;;
    a4) echo "a4-reverse.sh|Trace detail: Java producer/consumer link plus Java -> Rust notifications hop" ;;
    a12) echo "a12-cli-run.sh|Runs: command row with exit code; requires cargo build first" ;;
    a13) echo "a13-deploy-regression.sh|Issues: error spike attributed to service.version=v2" ;;
    a14) echo "a14-flag-flip.sh|Traces/Issues: feature_flag events and flag-scoped failures" ;;
    a18) echo "a18-canary.sh|Issues/Logs: redaction of fake email/token/card/jwt corpus" ;;
    a22) echo "a22-tokio-saturation.sh|Services -> checkout -> Runtime lane: tokio.runtime.* spike; Traces: slow checkout spans" ;;
    a27) echo "a27-execution-stack.sh|Runs/Story: stitched CLI -> daemon -> container -> agent beats; orphan child trace has browser_without_backend" ;;
    b-async-chaos) echo "b-async-chaos.sh|Services/Traces: lag span and dead-letter error branch" ;;
    b-chaos) echo "b-chaos.sh|Issues/Services: checkout error and slow-span rendering" ;;
    b-checkout-chaos) echo "b-checkout-chaos.sh|Traces: retry/timeout branch and N+1 waterfall" ;;
    b-degradation) echo "b-degradation.sh|Traces/Issues: degraded response and skewed span timing" ;;
    b17) echo "b17-cron.sh|Runs: cron success/fail/stuck outcome; requires cargo build first" ;;
    b17b) echo "b17b-cron-suite.sh|Runs: cron attrs, missing slot, duplicate cron.invocation.id; requires cargo build first" ;;
    b19) echo "b19-jvm-gc-pressure.sh|Services -> catalog -> Runtime lane: jvm.memory.used / jvm.gc.*; GraphQL spans slow" ;;
    b20) echo "b20-container-oom.sh|Docker OOM/restart + telemetry gap; destructive, pass --yes to the script" ;;
    b22) echo "b22-sampling-gap.sh|Traces: sampled-out gaps at 10 percent root sampling; Logs: full request evidence" ;;
    b23) echo "b23-uncorrelated-log.sh|Logs: orphan diagnostic without trace chip" ;;
    *) return 1 ;;
  esac
}

if [[ $# -eq 0 ]]; then
  catalog
  exit 0
fi

id="$1"
if ! entry="$(scenario "$id")"; then
  echo "Unknown scenario: $id" >&2
  echo >&2
  catalog >&2
  exit 2
fi

script="${entry%%|*}"
check="${entry#*|}"
if [[ "$id" == "a7" ]]; then
  bun "$SCRIPT_DIR/$script"
elif [[ "$id" == "b20" ]]; then
  "$SCRIPT_DIR/$script" --yes
else
  "$SCRIPT_DIR/$script"
fi
echo
echo "Check in Parallax UI: $check"
