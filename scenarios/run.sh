#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

catalog() {
  cat <<'TABLE'
ID              Script                         Drives                                      Check in Parallax UI
a1              a1-checkout.sh                 checkout -> pricing/inventory/recommendation -> outbox -> fulfillment Traces: checkout waterfall plus async fulfillment
a2              a2-exemplars.sh                catalog JVM exemplar traffic                  Metrics: trace-linked catalog counter exemplars
a5              a5-rum-error.sh                current Playwright catalog-to-checkout journey Traces: browser checkout journey and route evidence
a6              a6-graphql.sh                  catalog GraphQL field-span family            Traces: batched vs N+1, partial error, op-name policy
a7              a7-subscription.ts             catalog GraphQL subscription smoke            Traces: long-lived priceChanges subscription span
a8              a8-java-async.sh               authenticated seeded-order replay             Traces: Java producer/consumer link and Rust notification hop
a7b             a7b-grpc-stream.sh             pricing gRPC stream/rejection/cancel paths   Traces: rpc.message SENT/RECEIVED events and cancellation
a9              a9-field-spike.sh              CLI log-pattern corpus                      Logs: 20k templates with a late shape.template=spike
a10             a10-baggage.sh                  checkout W3C tenant/tier baggage             Traces: checkout, inventory, and pricing carry tenant.id/user.tier
a3              a3-async.sh                    checkout transactional outbox -> fulfillment Trace detail: checkout outbox producer linked to Java consumer
a4              a4-reverse.sh                  authenticated seeded-order replay             Trace detail: Java async span link and Java -> Rust hop
a12             a12-cli-run.sh                 playground CLI checkout driver              Runs: command row with exit code; cargo build first
a13             a13-deploy-regression.sh       checkout v1 then v2 release attribution     Traces: compare service.version=v1 versus v2 on valid traffic
a14             a14-flag-flip.sh               flagd checkoutFlow control/orchestrated     Traces: live feature variant changes checkout behavior
a18             a18-canary.sh                  Sentry envelope redaction corpus             Issues/Logs: redaction of fake email/token/card/jwt corpus
a19             a19-long-trace.sh              CLI wide-trace corpus                       Traces: 521-span trace for waterfall/minimap stress
a20-compare     a20-compare-pair.sh            checkoutFlow control/orchestrated pair      Traces: compare recommendation topology and feature variants
a20             a20-batch-fanin.sh             independent synthetic orders messages         Trace detail: synthetic producer/consumer links across concurrent messages
a22             a22-tokio-saturation.sh        bounded async checkout delay                Traces/Metrics: delayed concurrent checkout spans and active requests
a23             a23-storefront-grpc.sh         storefront GraphQL -> Pricing gRPC          Traces: GraphQL resolver then Pricing gRPC server
a24             a24-storefront-catalog.sh       storefront GraphQL -> catalog GraphQL       Traces: GraphQL resolver then catalog GraphQL operation
a25             a25-postgres.sh                 inventory real Postgres and pool pressure    Traces: db spans, pg_sleep, SELECT fan-out, pool_exhausted; Runtime: db.client.connection.*
a26             a26-cache.sh                    catalog-backed recommendation stampede      Traces: parallel Catalog requests and recommendation fan-out
a27             a27-execution-stack.sh         CLI -> daemon -> container -> agent          Runs/Story: stitched beats; orphan child trace has browser_without_backend
a28             a28-rum-journey.sh             browser routes, web vitals, session.id       Traces: stitched browser checkout, web vitals, orders, analytics
a29             a29-typed-events.sh            typed log events across tiers                Logs SQL/Event column: checkout/order/catalog/web event names
b-async-chaos   b-async-chaos.sh               synthetic orders lag and poison message      Services/Traces: synthetic lag span and dead-letter error branch
b2              b2-inventory-failure.sh        deterministic inventory 503                  Traces/Issues: inventory failure and checkout impact
b5              b5-cpu-pressure.sh             bounded checkout latency pressure             Services: checkout latency and active-request saturation
b6              b6-cache-leak.sh               recommendation cache-leak traffic             Metrics: memory growth plus feature-flag evaluation
b10             b10-lock-contention.sh         concurrent delayed checkout requests          Traces: concurrent bounded delay
b13             b13-slow-recommendation.sh     deterministic recommendation slowness          Traces: slow recommendation/degradation
b15             b15-rage-click.sh              current Playwright order/analytics journeys   RUM: browser route and API evidence
b16             b16-load.sh                    k6 checkout load entry                         Metrics/traces: sustained checkout load
b-chaos         b-chaos.sh                     payment failure and latency                  Issues/Services: checkout error and slow-span rendering
b-checkout-chaos b-checkout-chaos.sh           pricing retry/timeout and delayed checkout   Traces: retry/timeout branch and delayed request latency
b3b             b3b-grpc-deadline.sh           real grpc-timeout deadline and retry spans    Traces: rpc.grpc.status_code=4 on pricing.attempt spans
a-breach-error-rate a-breach-error-rate.sh      sustained checkout provider declines (>=3 min)  Alerts: high-error-rate incident opens for checkout
a-breach-p95    a-breach-p95.sh                sustained slow recommendation responses (>=3 min)             Alerts: p95 latency incident opens for recommendation
a-recover       a-recover.sh                   flag off + healthy traffic until incidents resolve            Alerts: incidents resolve; resolved webhook delivered
b-degradation   b-degradation.sh               provider-unavailable degrade and delay       Traces/Issues: degraded response and delayed checkout
b17             b17-cron.sh                    playground cron mode                         Runs: cron success/fail/stuck outcome; cargo build first
b17b            b17b-cron-suite.sh             cron ok/fail/stuck/missed/duplicate          Runs: schedule attrs, missing beat, duplicate invocation id
b19             b19-jvm-gc-pressure.sh         bounded catalog GraphQL workload             Services/Traces: products resolver spans and latency
b20             b20-container-oom.sh --yes     recommendation leak under 128m overlay       Docker OOM/restart + telemetry gap; destructive, requires --yes
b21             b21-orphan-consumer.sh          linked vs orphan synthetic orders consumer    Traces: linkless root consumer; Runtime: messaging.queue.depth
b22             b22-sampling-gap.sh            checkout at 10 percent root sampling         Traces: sampled-out gaps; Logs: full request evidence
b23             b23-uncorrelated-log.sh        correlated checkout request                  Logs: checkout rows retain trace/span context
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
eco-external    corner-cases.sh eco-external   corner-case corpus (plan 166)             Ecosystem: checkout CLIENT span to api.stripe.test with no instrumented SERVER side
e-burst         corner-cases.sh e-burst        corner-case corpus (plan 161)             Issues: one recurring error type plus five distinct error.type values
e-multi-lang    corner-cases.sh e-multi-lang   corner-case corpus (plan 161)             Issues: same failure with Rust/Java/browser fingerprints
p-grpc-err      corner-cases.sh p-grpc-err     corner-case corpus (plan 161)             Traces: gRPC OK/INVALID_ARGUMENT/DEADLINE_EXCEEDED variants
p-grpc-stream   corner-cases.sh p-grpc-stream  corner-case corpus (plan 161)             Traces: streaming RPC with per-message events
p-graphql-err   corner-cases.sh p-graphql-err  corner-case corpus (plan 161)             Traces: GraphQL field error with partial data + request-level error
p-rabbitmq-lag  corner-cases.sh p-rabbitmq-lag corner-case corpus (plan 161)             Traces: consumer lag + dead-letter over the RabbitMQ leg
j-happy         corner-cases.sh j-happy        corner-case corpus (plan 161)             CLI Apps journey: home→cart→checkout, all actions succeed
j-error         corner-cases.sh j-error        corner-case corpus (plan 161)             CLI Apps journey: checkout.submit fails on the checkout screen with widget context
j-outside       corner-cases.sh j-outside      corner-case corpus (plan 161)             CLI Apps journey: error between screen visits lands in the unattributed bucket
j-reattach      corner-cases.sh j-reattach     corner-case corpus (plan 161)             CLI Apps journey: three sessions chained via session.previous_id
j-parallel      corner-cases.sh j-parallel     corner-case corpus (plan 161)             CLI Apps: three concurrent console invocations + the daemon (four correlation domains)
eco-full        corner-cases.sh eco-full       corner-case corpus (plan 161)             Ecosystem: every edge with cli/browser/service node kinds present
c1              c1-issue-context.sh            issue + evidence bundle + resolve         Issues: bundle hash via GraphQL matches issue context
c2              c2-invocation-lifecycle.sh     invocation start/inspect/bundle           CLI Apps: wrapped invocation row
c3              c3-live-tail.sh                SSE logs+traces streams                   Logs/Traces live tail
c4              c4-alerting.sh                 rule + incident after error seed          Alerts: open incident
c5              c5-saved-state.sh              dashboard + investigation save            Dashboards / Investigations
c6              c6-github-ingest.sh            GitHub deploy webhook HMAC                Services deploy (needs github_* enabled)
c7              c7-agent-session.sh            import-claude + MCP tools                 Story / MCP issue_context + agent_session_show
c8              c8-sentry-envelope.sh          real Rust/Java/JS SDK envelopes           Issues from each SDK
c9              c9-lifecycle-ops.sh            isolated-HOME doctor/prune + context      doctor / prune / contexts / --otlp-forward
c10             c10-redaction-egress.sh        canary absent on every egress             bundle/MCP/UI/webhook/Sentry ack
c11             c11-ui-agent-verify.sh        agent-browser snapshots for every core surface /health green  Overview, Issues, Traces, Logs, Metrics, Services, Ecosystem, Invocations, Alerts, Dashboards, Investigations, SQL, Tests
a30             a30-metric-shapes.sh           checkout request metrics                   Metrics: http.server.active_requests and request latency
a31             a31-handled-unhandled.sh       payment decline vs provider internal       Issues: handled 402 decline versus handled 502 internal
TABLE
}

scenario() {
  case "$1" in
    a1) echo "a1-checkout.sh|Traces: checkout waterfall with pricing, inventory, recommendation, and transactional-outbox fulfillment" ;;
    a2) echo "a2-exemplars.sh|Metrics: trace-linked catalog.product.queries exemplars" ;;
    a5) echo "a5-rum-error.sh|Traces: current browser catalog-to-checkout journey" ;;
    a6) echo "a6-graphql.sh|Traces: batched reviews vs reviewsSlow N+1 shape, partial riskScore error, op-name policy" ;;
    a7) echo "a7-subscription.ts|Traces: long-lived priceChanges subscription span; run with Bun" ;;
    a7b) echo "a7b-grpc-stream.sh|Traces: pricing stream SENT/RECEIVED events, product rejection, and cancellation observation" ;;
    a8) echo "a8-java-async.sh|Traces: authenticated seeded-order replay, Java producer/consumer link, and Rust notification hop" ;;
    a9) echo "a9-field-spike.sh|Logs: current CLI pattern corpus with a late shape.template=spike" ;;
    a10) echo "a10-baggage.sh|Traces: checkout, inventory, and pricing carry tenant.id/user.tier via W3C baggage" ;;
    a3) echo "a3-async.sh|Trace detail: checkout transactional outbox producer with link to Java fulfillment consumer" ;;
    a4) echo "a4-reverse.sh|Trace detail: authenticated seeded-order replay, Java link, and Java -> Rust notifications hop" ;;
    a12) echo "a12-cli-run.sh|Runs: command row with exit code; requires cargo build first" ;;
    a13) echo "a13-deploy-regression.sh|Traces: compare valid checkout traffic attributed to service.version=v1 versus v2" ;;
    a14) echo "a14-flag-flip.sh|Traces: feature_flag events and live checkoutFlow topology changes" ;;
    a18) echo "a18-canary.sh|Issues/Logs: redaction of fake email/token/card/jwt corpus" ;;
    a19) echo "a19-long-trace.sh|Traces: current CLI 521-span wide trace for waterfall windowing and minimap stress" ;;
    a20-compare) echo "a20-compare-pair.sh|Traces: compare checkoutFlow control versus orchestrated recommendation topology" ;;
    a20) echo "a20-batch-fanin.sh|Trace detail: independent synthetic orders producer/consumer links across concurrent messages" ;;
    a22) echo "a22-tokio-saturation.sh|Traces/Metrics: delayed concurrent checkout spans and active-request pressure" ;;
    a23) echo "a23-storefront-grpc.sh|Traces: storefront GraphQL resolver then Pricing gRPC server" ;;
    a24) echo "a24-storefront-catalog.sh|Traces: storefront GraphQL resolver then Java catalog GraphQL operation" ;;
    a25) echo "a25-postgres.sh|Traces: db.query.text spans for reserve, pg_sleep, SELECT fan-out, and pool_exhausted; Runtime: db.client.connection.* gauges" ;;
    a26) echo "a26-cache.sh|Traces: Catalog-backed recommendation fan-out and bounded stampede workers" ;;
    a27) echo "a27-execution-stack.sh|Runs/Story: stitched CLI -> daemon -> container -> agent beats; orphan child trace has browser_without_backend" ;;
    a28) echo "a28-rum-journey.sh|Traces: browser route/user-step spans, web_vital spans, stitched checkout, orders, analytics, and web.checkout.submitted" ;;
    a29) echo "a29-typed-events.sh|Logs SQL/Event column: checkout.completed, order.consumed, catalog.products.served, and web.checkout.submitted" ;;
    b-async-chaos) echo "b-async-chaos.sh|Services/Traces: synthetic orders lag span and dead-letter error branch" ;;
    b2) echo "b2-inventory-failure.sh|Traces/Issues: inventory failure and checkout impact" ;;
    b5) echo "b5-cpu-pressure.sh|Services: checkout latency, active requests, and slow request spans" ;;
    b6) echo "b6-cache-leak.sh|Metrics: recommendation memory growth and feature-flag evaluation" ;;
    b10) echo "b10-lock-contention.sh|Traces: concurrent checkout spans through bounded delay" ;;
    b13) echo "b13-slow-recommendation.sh|Traces: bounded slow recommendation latency" ;;
    b15) echo "b15-rage-click.sh|RUM: current Playwright order and analytics route evidence" ;;
    b16) echo "b16-load.sh|Metrics/traces: sustained checkout load from k6" ;;
    b-chaos) echo "b-chaos.sh|Issues/Services: checkout error and slow-span rendering" ;;
    b-checkout-chaos) echo "b-checkout-chaos.sh|Traces: pricing retry/timeout and delayed checkout paths" ;;
    b3b) echo "b3b-grpc-deadline.sh|Traces: pricing.attempt sibling spans carry rpc.grpc.status_code=4 and deadline_exceeded" ;;
    a-breach-error-rate) echo "a-breach-error-rate.sh|Alerts: sustained checkout failures open a high-error-rate incident" ;;
    a-breach-p95) echo "a-breach-p95.sh|Alerts: sustained slow recommendation opens a p95 latency incident" ;;
    a-recover) echo "a-recover.sh|Alerts: healthy traffic resolves open incidents; resolved webhook delivered" ;;
    b-degradation) echo "b-degradation.sh|Traces/Issues: provider-unavailable degraded response and delayed checkout" ;;
    b17) echo "b17-cron.sh|Runs: cron success/fail/stuck outcome; requires cargo build first" ;;
    b17b) echo "b17b-cron-suite.sh|CLI Apps: cron attrs, missing slot, duplicate firings sharing one cli.invocation.id; requires cargo build first" ;;
    b19) echo "b19-jvm-gc-pressure.sh|Services/Traces: catalog products resolver spans and latency" ;;
    b20) echo "b20-container-oom.sh|Docker OOM/restart + telemetry gap; destructive, pass --yes to the script" ;;
    b21) echo "b21-orphan-consumer.sh|Traces: synthetic orders consumer link/orphan roots; Runtime: messaging.queue.depth rises" ;;
    b22) echo "b22-sampling-gap.sh|Traces: sampled-out gaps at 10 percent root sampling; Logs: full request evidence" ;;
    b23) echo "b23-uncorrelated-log.sh|Logs: checkout rows retain normal trace/span correlation" ;;
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
    eco-external) echo "corner-cases.sh eco-external|Ecosystem: checkout CLIENT span to api.stripe.test with no instrumented SERVER side" ;;
    e-burst) echo "corner-cases.sh e-burst|Issues: one recurring error type plus five distinct error.type values" ;;
    e-multi-lang) echo "corner-cases.sh e-multi-lang|Issues: same failure with Rust/Java/browser fingerprints" ;;
    p-grpc-err) echo "corner-cases.sh p-grpc-err|Traces: successful pricing leg, HTTP validation, and pricing deadline" ;;
    p-grpc-stream) echo "corner-cases.sh p-grpc-stream|Traces: streaming RPC with per-message events" ;;
    p-graphql-err) echo "corner-cases.sh p-graphql-err|Traces: GraphQL field error with partial data + request-level error" ;;
    p-rabbitmq-lag) echo "corner-cases.sh p-rabbitmq-lag|Traces: consumer lag + dead-letter over the RabbitMQ leg" ;;
    j-happy) echo "corner-cases.sh j-happy|CLI Apps journey: home→cart→checkout, all actions succeed" ;;
    j-error) echo "corner-cases.sh j-error|CLI Apps journey: checkout.submit fails on the checkout screen with widget context" ;;
    j-outside) echo "corner-cases.sh j-outside|CLI Apps journey: error between screen visits lands in the unattributed bucket" ;;
    j-reattach) echo "corner-cases.sh j-reattach|CLI Apps journey: three sessions chained via session.previous_id" ;;
    j-parallel) echo "corner-cases.sh j-parallel|CLI Apps: three concurrent console invocations + the daemon (four correlation domains)" ;;
    eco-full) echo "corner-cases.sh eco-full|Ecosystem: every edge with cli/browser/service node kinds present" ;;
    c1) echo "c1-issue-context.sh|Issues: bundle hash via GraphQL matches issue context" ;;
    c2) echo "c2-invocation-lifecycle.sh|CLI Apps: wrapped invocation row" ;;
    c3) echo "c3-live-tail.sh|Logs/Traces live tail streams" ;;
    c4) echo "c4-alerting.sh|Alerts: open incident after error seed" ;;
    c5) echo "c5-saved-state.sh|Dashboards / Investigations saved" ;;
    c6) echo "c6-github-ingest.sh|GitHub deploy webhook HMAC" ;;
    c7) echo "c7-agent-session.sh|Story / MCP issue_context + agent_session_show" ;;
    c8) echo "c8-sentry-envelope.sh|Issues from real Rust/Java/JS Sentry SDKs" ;;
    c9) echo "c9-lifecycle-ops.sh|isolated-HOME doctor/prune + context + --otlp-forward" ;;
    c10) echo "c10-redaction-egress.sh|canary absent on bundle/MCP/UI/webhook/Sentry ack" ;;
    c11) echo "c11-ui-agent-verify.sh|Overview, Issues, Traces, Logs, Metrics, Services, Ecosystem, Invocations, Alerts, Dashboards, Investigations, SQL, and Tests are non-blank" ;;
    a30) echo "a30-metric-shapes.sh|Metrics: http.server.active_requests, request latency, and checkout service metrics" ;;
    a31) echo "a31-handled-unhandled.sh|Issues: handled payment decline 402 versus handled provider internal 502" ;;
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
