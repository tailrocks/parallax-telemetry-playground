#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

catalog() {
  cat <<'TABLE'
ID              Script                         Drives                                      Check in Parallax UI
a1              a1-checkout.sh                 checkout -> pricing/inventory/recommendation Traces: checkout waterfall with downstream children
a2              a2-exemplars.sh                catalog JVM exemplar traffic                  Metrics: trace-linked catalog counter exemplars
a5              a5-rum-error.sh                Playwright forced RUM error                   Traces/Issues: browser failure with replay/error evidence
a6              a6-graphql.sh                  catalog GraphQL field-span family            Traces: batched vs N+1, partial error, op-name policy
a7              a7-subscription.ts             catalog GraphQL subscription smoke            Traces: long-lived priceChanges subscription span
a8              a8-java-async.sh               Java fulfillment Kafka link path              Traces: Java producer/consumer link and Rust notification hop
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
a23             a23-storefront-grpc.sh          storefront GraphQL -> Java gRPC             Traces: GraphQL resolver then payment Pricing/Quote
a24             a24-storefront-catalog.sh       storefront GraphQL -> catalog GraphQL       Traces: GraphQL resolver then catalog GraphQL operation
a25             a25-postgres.sh                 inventory real Postgres and pool pressure    Traces: db spans, pg_sleep, SELECT fan-out, pool_exhausted; Runtime: db.client.connection.*
a26             a26-cache.sh                    recommendation TTL cache and stampede       Metrics: cache_hits_total/cache_misses_total; Traces: parallel compute_recommendations
a27             a27-execution-stack.sh         CLI -> daemon -> container -> agent          Runs/Story: stitched beats; orphan child trace has browser_without_backend
a28             a28-rum-journey.sh             browser routes, web vitals, session.id       Traces: stitched browser checkout, RUM error, web_vital spans, nopropagate gap
a29             a29-typed-events.sh            typed log events across tiers                Logs SQL/Event column: checkout/order/catalog/payment/web event names
b-async-chaos   b-async-chaos.sh               consumer lag and poison message              Services/Traces: lag span and dead-letter error branch
b2              b2-inventory-failure.sh        deterministic inventory 503                  Traces/Issues: inventory failure and checkout impact
b5              b5-cpu-pressure.sh             bounded checkout CPU hot path                 Services: checkout CPU/runtime saturation
b6              b6-cache-leak.sh               recommendation cache-leak traffic             Metrics: memory growth plus feature-flag evaluation
b10             b10-lock-contention.sh         concurrent shared-lock checkout requests      Traces: serialized contention delay
b13             b13-slow-recommendation.sh     deterministic recommendation slowness          Traces: slow recommendation/degradation
b15             b15-rage-click.sh              Playwright checkout/promo journey             RUM: rage-click/replay evidence
b16             b16-load.sh                    k6 checkout load entry                         Metrics/traces: sustained checkout load
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
t-deep          corner-cases.sh t-deep         corner-case corpus (plan 161)             Traces: 14-span linear chain across three simulated tiers (depth rendering)
t-wide          corner-cases.sh t-wide         corner-case corpus (plan 161)             Traces: 521-span fan-out in one trace (virtualization, minimap sampling)
t-multiroot     corner-cases.sh t-multiroot    corner-case corpus (plan 161)             Traces: one trace id with two root spans; both must render
t-orphan        corner-cases.sh t-orphan       corner-case corpus (plan 161)             Traces: detached child whose parent never arrives (renders, not vanishes)
t-skew          corner-cases.sh t-skew         corner-case corpus (plan 161)             Traces: SERVER child starts before its CLIENT parent (skew banner, non-negative bars)
t-zero          corner-cases.sh t-zero         corner-case corpus (plan 161)             Traces: zero-duration span and a 1µs twin (no divide-by-zero, visible bars)
t-links         corner-cases.sh t-links        corner-case corpus (plan 161)             Traces: two traces cross-linked both ways (link navigation)
t-longnames     corner-cases.sh t-longnames    corner-case corpus (plan 161)             Traces: 1-4 KiB unicode/emoji names and values (truncation, copy)
t-events        corner-cases.sh t-events       corner-case corpus (plan 161)             Trace detail: 51 span events incl. Rust/Java/browser stacktraces
l-burst         corner-cases.sh l-burst        corner-case corpus (plan 161)             Logs: 5k logs in seconds (live tail caps, histogram)
l-bodies        corner-cases.sh l-bodies       corner-case corpus (plan 161)             Logs: JSON body, 32 KiB body, ANSI escapes, blank body, identical timestamps
l-patterns      corner-cases.sh l-patterns     corner-case corpus (plan 165)             Logs: 20k lines from 12 templates with parameter churn + one late spiking template (Drain clustering)
m-shapes        corner-cases.sh m-shapes       corner-case corpus (plan 161)             Metrics: counter reset mid-window, gauge gap, exemplar-bearing histogram
m-labels        corner-cases.sh m-labels       corner-case corpus (plan 168)             Metrics: gauge + sum with region label eu/us/ap at fixed 6/3/1 proportions (group-by assertions)
f-attrs         corner-cases.sh f-attrs        corner-case corpus (plan 164)             Traces/Logs: 100 spans + 100 logs with http.request.method 70/20/10 GET/POST/DELETE (facet count assertions)
e-burst         corner-cases.sh e-burst        corner-case corpus (plan 161)             Issues: one recurring error type plus five distinct error.type values
e-multi-lang    corner-cases.sh e-multi-lang   corner-case corpus (plan 161)             Issues: same failure with Rust/Java/browser fingerprints
p-grpc-err      corner-cases.sh p-grpc-err     corner-case corpus (plan 161)             Traces: gRPC OK/INVALID_ARGUMENT/DEADLINE_EXCEEDED variants
p-grpc-stream   corner-cases.sh p-grpc-stream  corner-case corpus (plan 161)             Traces: streaming RPC with per-message events
p-graphql-err   corner-cases.sh p-graphql-err  corner-case corpus (plan 161)             Traces: GraphQL field error with partial data + request-level error
p-kafka-lag     corner-cases.sh p-kafka-lag    corner-case corpus (plan 161)             Traces: consumer lag + dead-letter over the Kafka leg
j-happy         corner-cases.sh j-happy        corner-case corpus (plan 161)             CLI Apps journey: home→cart→checkout, all actions succeed
j-error         corner-cases.sh j-error        corner-case corpus (plan 161)             CLI Apps journey: checkout.submit fails on the checkout screen with widget context
j-outside       corner-cases.sh j-outside      corner-case corpus (plan 161)             CLI Apps journey: error between screen visits lands in the unattributed bucket
j-reattach      corner-cases.sh j-reattach     corner-case corpus (plan 161)             CLI Apps journey: three sessions chained via session.previous_id
j-parallel      corner-cases.sh j-parallel     corner-case corpus (plan 161)             CLI Apps: three concurrent console invocations + the daemon (four correlation domains)
eco-full        corner-cases.sh eco-full       corner-case corpus (plan 161)             Ecosystem: every edge with cli/browser/service node kinds present
TABLE
}

scenario() {
  case "$1" in
    a1) echo "a1-checkout.sh|Traces: checkout waterfall with pricing, inventory, and recommendation children" ;;
    a2) echo "a2-exemplars.sh|Metrics: trace-linked catalog.product.queries exemplars" ;;
    a5) echo "a5-rum-error.sh|Traces/Issues: forced RUM error with browser test evidence" ;;
    a6) echo "a6-graphql.sh|Traces: batched reviews vs reviewsSlow N+1 shape, partial riskScore error, op-name policy" ;;
    a7) echo "a7-subscription.ts|Traces: long-lived priceChanges subscription span; run with Bun" ;;
    a7b) echo "a7b-grpc-stream.sh|Traces: pricing stream SENT events, checkout RECEIVED events, stream_failed, and cancel observation" ;;
    a8) echo "a8-java-async.sh|Traces: Java fulfillment producer/consumer link and Rust notification hop" ;;
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
    a23) echo "a23-storefront-grpc.sh|Traces: storefront GraphQL resolver then Java payment Pricing/Quote gRPC server" ;;
    a24) echo "a24-storefront-catalog.sh|Traces: storefront GraphQL resolver then Java catalog GraphQL operation" ;;
    a25) echo "a25-postgres.sh|Traces: db.query.text spans for reserve, pg_sleep, SELECT fan-out, and pool_exhausted; Runtime: db.client.connection.* gauges" ;;
    a26) echo "a26-cache.sh|Metrics: cache_hits_total/cache_misses_total and cache_size; Traces: parallel compute_recommendations spans; Logs document fields: cache.hit" ;;
    a27) echo "a27-execution-stack.sh|Runs/Story: stitched CLI -> daemon -> container -> agent beats; orphan child trace has browser_without_backend" ;;
    a28) echo "a28-rum-journey.sh|Traces: browser route/user-step spans, web_vital spans, stitched checkout, RUM exception, and nopropagate disconnected-trace gap" ;;
    a29) echo "a29-typed-events.sh|Logs SQL/Event column: checkout.completed, checkout.failed, order.consumed, catalog.products.served, payment.authorized, and web.checkout.submitted" ;;
    b-async-chaos) echo "b-async-chaos.sh|Services/Traces: lag span and dead-letter error branch" ;;
    b2) echo "b2-inventory-failure.sh|Traces/Issues: inventory failure and checkout impact" ;;
    b5) echo "b5-cpu-pressure.sh|Services: checkout CPU/runtime saturation and slow request spans" ;;
    b6) echo "b6-cache-leak.sh|Metrics: recommendation memory growth and feature-flag evaluation" ;;
    b10) echo "b10-lock-contention.sh|Traces: serialized checkout contention delay" ;;
    b13) echo "b13-slow-recommendation.sh|Traces: slow recommendation and dependent degradation" ;;
    b15) echo "b15-rage-click.sh|RUM: rage-click/replay evidence from the Playwright journey" ;;
    b16) echo "b16-load.sh|Metrics/traces: sustained checkout load from k6" ;;
    b-chaos) echo "b-chaos.sh|Issues/Services: checkout error and slow-span rendering" ;;
    b-checkout-chaos) echo "b-checkout-chaos.sh|Traces: retry/timeout branch and N+1 waterfall" ;;
    b3b) echo "b3b-grpc-deadline.sh|Traces: pricing.attempt sibling spans carry rpc.grpc.status_code=4 and deadline_exceeded" ;;
    b-degradation) echo "b-degradation.sh|Traces/Issues: degraded response and skewed span timing" ;;
    b17) echo "b17-cron.sh|Runs: cron success/fail/stuck outcome; requires cargo build first" ;;
    b17b) echo "b17b-cron-suite.sh|CLI Apps: cron attrs, missing slot, duplicate firings sharing one cli.invocation.id; requires cargo build first" ;;
    b19) echo "b19-jvm-gc-pressure.sh|Services -> catalog -> Runtime lane: jvm.memory.used / jvm.gc.*; GraphQL spans slow" ;;
    b20) echo "b20-container-oom.sh|Docker OOM/restart + telemetry gap; destructive, pass --yes to the script" ;;
    b21) echo "b21-orphan-consumer.sh|Traces: normal consumer has link, orphan consumer is linkless root; Runtime: messaging.queue.depth rises" ;;
    b22) echo "b22-sampling-gap.sh|Traces: sampled-out gaps at 10 percent root sampling; Logs: full request evidence" ;;
    b23) echo "b23-uncorrelated-log.sh|Logs: orphan diagnostic without trace chip" ;;
    t-deep) echo "corner-cases.sh t-deep|Traces: 14-span linear chain across three simulated tiers (depth rendering)" ;;
    t-wide) echo "corner-cases.sh t-wide|Traces: 521-span fan-out in one trace (virtualization, minimap sampling)" ;;
    t-multiroot) echo "corner-cases.sh t-multiroot|Traces: one trace id with two root spans; both must render" ;;
    t-orphan) echo "corner-cases.sh t-orphan|Traces: detached child whose parent never arrives (renders, not vanishes)" ;;
    t-skew) echo "corner-cases.sh t-skew|Traces: SERVER child starts before its CLIENT parent (skew banner, non-negative bars)" ;;
    t-zero) echo "corner-cases.sh t-zero|Traces: zero-duration span and a 1µs twin (no divide-by-zero, visible bars)" ;;
    t-links) echo "corner-cases.sh t-links|Traces: two traces cross-linked both ways (link navigation)" ;;
    t-longnames) echo "corner-cases.sh t-longnames|Traces: 1-4 KiB unicode/emoji names and values (truncation, copy)" ;;
    t-events) echo "corner-cases.sh t-events|Trace detail: 51 span events incl. Rust/Java/browser stacktraces" ;;
    l-burst) echo "corner-cases.sh l-burst|Logs: 5k logs in seconds (live tail caps, histogram)" ;;
    l-bodies) echo "corner-cases.sh l-bodies|Logs: JSON body, 32 KiB body, ANSI escapes, blank body, identical timestamps" ;;
    l-patterns) echo "corner-cases.sh l-patterns|Logs: 20k lines from 12 templates with parameter churn + one late spiking template (Drain clustering)" ;;
    m-shapes) echo "corner-cases.sh m-shapes|Metrics: counter reset mid-window, gauge gap, exemplar-bearing histogram" ;;
    m-labels) echo "corner-cases.sh m-labels|Metrics: gauge + sum with region label eu/us/ap at fixed 6/3/1 proportions (group-by assertions)" ;;
    f-attrs) echo "corner-cases.sh f-attrs|Traces/Logs: 100 spans + 100 logs with http.request.method 70/20/10 GET/POST/DELETE (facet count assertions)" ;;
    e-burst) echo "corner-cases.sh e-burst|Issues: one recurring error type plus five distinct error.type values" ;;
    e-multi-lang) echo "corner-cases.sh e-multi-lang|Issues: same failure with Rust/Java/browser fingerprints" ;;
    p-grpc-err) echo "corner-cases.sh p-grpc-err|Traces: gRPC OK/INVALID_ARGUMENT/DEADLINE_EXCEEDED variants" ;;
    p-grpc-stream) echo "corner-cases.sh p-grpc-stream|Traces: streaming RPC with per-message events" ;;
    p-graphql-err) echo "corner-cases.sh p-graphql-err|Traces: GraphQL field error with partial data + request-level error" ;;
    p-kafka-lag) echo "corner-cases.sh p-kafka-lag|Traces: consumer lag + dead-letter over the Kafka leg" ;;
    j-happy) echo "corner-cases.sh j-happy|CLI Apps journey: home→cart→checkout, all actions succeed" ;;
    j-error) echo "corner-cases.sh j-error|CLI Apps journey: checkout.submit fails on the checkout screen with widget context" ;;
    j-outside) echo "corner-cases.sh j-outside|CLI Apps journey: error between screen visits lands in the unattributed bucket" ;;
    j-reattach) echo "corner-cases.sh j-reattach|CLI Apps journey: three sessions chained via session.previous_id" ;;
    j-parallel) echo "corner-cases.sh j-parallel|CLI Apps: three concurrent console invocations + the daemon (four correlation domains)" ;;
    eco-full) echo "corner-cases.sh eco-full|Ecosystem: every edge with cli/browser/service node kinds present" ;;
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
# The script field may carry fixed arguments (e.g. "corner-cases.sh t-deep").
read -r script_file script_args <<<"$script"
if [[ "$id" == "a7" ]]; then
  bun "$SCRIPT_DIR/$script_file" "$@"
elif [[ "$script_file" == "requires-live-host" ]]; then
  echo "Scenario $id requires a live host: $check"
elif [[ -n "$script_args" ]]; then
  "$SCRIPT_DIR/$script_file" $script_args "$@"
else
  "$SCRIPT_DIR/$script_file" "$@"
fi
echo
echo "Check in Parallax UI: $check"
