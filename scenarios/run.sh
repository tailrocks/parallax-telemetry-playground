#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

catalog() {
  cat <<'TABLE'
ID              Script                         Drives                                      Check in Parallax UI
a1              a1-checkout.sh                 checkout -> pricing/inventory/recommendation Traces: checkout waterfall with downstream children
a6              a6-graphql.sh                  catalog GraphQL field-span family            Traces: batched vs N+1, partial error, op-name policy
a7              a7-subscription.ts             catalog GraphQL subscription smoke            Traces: long-lived priceChanges subscription span
a7b             a7b-grpc-stream.sh             pricing gRPC stream events/failure/cancel    Traces: rpc.message SENT/RECEIVED events and stream_failed/cancel
a9              a9-field-spike.sh              checkout structured log burst                Logs document fields: app_screen_name dominated by workspace-select
a10             a10-baggage.sh                  checkout W3C tenant/tier baggage             Traces: checkout, inventory, and pricing carry tenant.id/user.tier
a3              a3-async.sh                    orders producer/consumer                    Trace detail: producer span linked to consumer trace
a4              a4-reverse.sh                  fulfillment -> Kafka -> notifications        Trace detail: Java async span link and Java -> Rust hop
a12             a12-cli-run.sh                 playground CLI checkout driver              Runs: command row with exit code; cargo build first
a13             a13-deploy-regression.sh       checkout v1 then v2 recreate                Issues: error spike attributed to service.version=v2
a14             a14-flag-flip.sh               flagd paymentFailure off/on/off              Traces/Issues: feature_flag events and flag-scoped failures
a18             a18-canary.sh                  fake sensitive canary corpus                 Issues/Logs: redaction of fake email/token/card/jwt corpus
a19             a19-long-trace.sh              checkout synthetic wide/deep trace           Traces: large burst span tree for waterfall/minimap stress
a20-compare     a20-compare-pair.sh            green structural checkout variants           Traces: Compare shows added reserve spans and removed recommend
a20             a20-batch-fanin.sh             orders batch consumer links many producers   Trace detail: consume_batch has many span links
a22             a22-tokio-saturation.sh        checkout spawn_blocking flood                Services -> checkout -> Runtime lane: tokio.runtime.* spike; Traces: slow spans
a25             a25-postgres.sh                 inventory real Postgres and pool pressure    Traces: db spans, pg_sleep, SELECT fan-out, pool_exhausted; Runtime: db.client.connection.*
a26             a26-cache.sh                    recommendation TTL cache and stampede       Metrics: cache_hits_total/cache_misses_total; Traces: parallel compute_recommendations
a27             a27-execution-stack.sh         CLI -> daemon -> container -> agent          Runs/Story: stitched beats; orphan child trace has browser_without_backend
a28             a28-rum-journey.sh             browser routes, web vitals, session.id       Traces: stitched browser checkout, RUM error, web_vital spans, nopropagate gap
a29             a29-typed-events.sh            typed log events across tiers                Logs SQL/Event column: checkout/order/catalog/payment/web event names
b-async-chaos   b-async-chaos.sh               consumer lag and poison message              Services/Traces: lag span and dead-letter error branch
b-chaos         b-chaos.sh                     payment failure and latency                  Issues/Services: checkout error and slow-span rendering
b-checkout-chaos b-checkout-chaos.sh           retry timeout and N+1 fan-out                Traces: retry/timeout branch and N+1 waterfall
b3b             b3b-grpc-deadline.sh           real grpc-timeout deadline and retry spans    Traces: rpc.grpc.status_code=4 on pricing.attempt spans
b-degradation   b-degradation.sh               partial degrade and skew                     Traces/Issues: degraded response and skewed span timing
b17             b17-cron.sh                    playground cron mode                         Runs: cron success/fail/stuck outcome; cargo build first
b17b            b17b-cron-suite.sh             cron ok/fail/stuck/missed/duplicate          Runs: schedule attrs, missing beat, duplicate invocation id
b19             b19-jvm-gc-pressure.sh         catalog bounded heap pressure                Services -> catalog -> Runtime lane: jvm.memory.used / jvm.gc.*; GraphQL spans slow
b20             b20-container-oom.sh --yes     recommendation leak under 128m overlay       Docker OOM/restart + telemetry gap; destructive, requires --yes
b21             b21-orphan-consumer.sh          linked vs orphan consumer plus lag gauge     Traces: linkless root consumer; Runtime: messaging.queue.depth
b22             b22-sampling-gap.sh            checkout at 10 percent root sampling         Traces: sampled-out gaps; Logs: full request evidence
b23             b23-uncorrelated-log.sh        detached checkout log outside span context   Logs: error row without trace chip
TABLE
}

scenario() {
  case "$1" in
    a1) echo "a1-checkout.sh|Traces: checkout waterfall with pricing, inventory, and recommendation children" ;;
    a6) echo "a6-graphql.sh|Traces: batched reviews vs reviewsSlow N+1 shape, partial riskScore error, op-name policy" ;;
    a7) echo "a7-subscription.ts|Traces: long-lived priceChanges subscription span; run with Bun" ;;
    a7b) echo "a7b-grpc-stream.sh|Traces: pricing stream SENT events, checkout RECEIVED events, stream_failed, and cancel observation" ;;
    a9) echo "a9-field-spike.sh|Logs document fields: app_screen_name dominated by workspace-select in the spike window" ;;
    a10) echo "a10-baggage.sh|Traces: checkout, inventory, and pricing carry tenant.id/user.tier via W3C baggage" ;;
    a3) echo "a3-async.sh|Trace detail: producer span with link to consumer trace" ;;
    a4) echo "a4-reverse.sh|Trace detail: Java producer/consumer link plus Java -> Rust notifications hop" ;;
    a12) echo "a12-cli-run.sh|Runs: command row with exit code; requires cargo build first" ;;
    a13) echo "a13-deploy-regression.sh|Issues: error spike attributed to service.version=v2" ;;
    a14) echo "a14-flag-flip.sh|Traces/Issues: feature_flag events and flag-scoped failures" ;;
    a18) echo "a18-canary.sh|Issues/Logs: redaction of fake email/token/card/jwt corpus" ;;
    a19) echo "a19-long-trace.sh|Traces: large burst span tree for waterfall windowing, minimap, and lane stress" ;;
    a20-compare) echo "a20-compare-pair.sh|Traces: Compare shows added reserve spans, removed recommend branch, and duration deltas" ;;
    a20) echo "a20-batch-fanin.sh|Trace detail: consume_batch span carries messaging.batch.message_count and many span links" ;;
    a22) echo "a22-tokio-saturation.sh|Services -> checkout -> Runtime lane: tokio.runtime.* spike; Traces: slow checkout spans" ;;
    a25) echo "a25-postgres.sh|Traces: db.query.text spans for reserve, pg_sleep, SELECT fan-out, and pool_exhausted; Runtime: db.client.connection.* gauges" ;;
    a26) echo "a26-cache.sh|Metrics: cache_hits_total/cache_misses_total and cache_size; Traces: parallel compute_recommendations spans; Logs document fields: cache.hit" ;;
    a27) echo "a27-execution-stack.sh|Runs/Story: stitched CLI -> daemon -> container -> agent beats; orphan child trace has browser_without_backend" ;;
    a28) echo "a28-rum-journey.sh|Traces: browser route/user-step spans, web_vital spans, stitched checkout, RUM exception, and nopropagate disconnected-trace gap" ;;
    a29) echo "a29-typed-events.sh|Logs SQL/Event column: checkout.completed, checkout.failed, order.consumed, catalog.products.served, payment.authorized, and web.checkout.submitted" ;;
    b-async-chaos) echo "b-async-chaos.sh|Services/Traces: lag span and dead-letter error branch" ;;
    b-chaos) echo "b-chaos.sh|Issues/Services: checkout error and slow-span rendering" ;;
    b-checkout-chaos) echo "b-checkout-chaos.sh|Traces: retry/timeout branch and N+1 waterfall" ;;
    b3b) echo "b3b-grpc-deadline.sh|Traces: pricing.attempt sibling spans carry rpc.grpc.status_code=4 and deadline_exceeded" ;;
    b-degradation) echo "b-degradation.sh|Traces/Issues: degraded response and skewed span timing" ;;
    b17) echo "b17-cron.sh|Runs: cron success/fail/stuck outcome; requires cargo build first" ;;
    b17b) echo "b17b-cron-suite.sh|Runs: cron attrs, missing slot, duplicate cron.invocation.id; requires cargo build first" ;;
    b19) echo "b19-jvm-gc-pressure.sh|Services -> catalog -> Runtime lane: jvm.memory.used / jvm.gc.*; GraphQL spans slow" ;;
    b20) echo "b20-container-oom.sh|Docker OOM/restart + telemetry gap; destructive, pass --yes to the script" ;;
    b21) echo "b21-orphan-consumer.sh|Traces: normal consumer has link, orphan consumer is linkless root; Runtime: messaging.queue.depth rises" ;;
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
shift
if ! entry="$(scenario "$id")"; then
  echo "Unknown scenario: $id" >&2
  echo >&2
  catalog >&2
  exit 2
fi

script="${entry%%|*}"
check="${entry#*|}"
if [[ "$id" == "a7" ]]; then
  bun "$SCRIPT_DIR/$script" "$@"
else
  "$SCRIPT_DIR/$script" "$@"
fi
echo
echo "Check in Parallax UI: $check"
