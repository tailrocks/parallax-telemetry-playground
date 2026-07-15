# Scenarios

Run `./scenarios/run.sh` for the catalog or `./scenarios/run.sh a1` for one
scenario. The table lists the existing scripts only; later scenario plans append
rows here and in `run.sh`.

| ID | Script | Drives | Check in Parallax UI |
|---|---|---|---|
| a1 | `a1-checkout.sh` | Checkout through pricing, inventory, and recommendation. | Traces: one checkout waterfall with pricing, inventory, and recommendation children. |
| a2 | `a2-exemplars.sh` | Catalog JVM metric traffic with trace-linked exemplars. | Metrics: `catalog.product.queries` points link back to catalog traces. |
| a5 | `a5-rum-error.sh` | Playwright forces the browser RUM error journey. | Traces/Issues: browser failure carries replay and test evidence. |
| a6 | `a6-graphql.sh` | Catalog GraphQL products queries: batched reviews, `reviewsSlow` N+1, partial `riskScore` error, and random operation name. | Traces: batched shape has one reviews/DataLoader fetch; N+1 has one `reviewsSlow` field span per product; partial error stays HTTP 200 with field error; op-name trace stays low-cardinality. |
| a7 | `a7-subscription.ts` | Catalog `priceChanges` GraphQL-over-WebSocket subscription via Bun native WebSocket. | Traces: long-lived subscription / data-fetcher span emits price events. |
| a7b | `a7b-grpc-stream.sh` | Pricing server-stream clean run, mid-stream `fail_at`, and client-side cancellation. | Traces: pricing stream span has `rpc.message` SENT events, checkout has RECEIVED events, failed run marks `stream_failed`, and cancel is observed server-side. |
| a8 | `a8-java-async.sh` | Java fulfillment exercises its Kafka producer/consumer link path. | Traces: Java producer and consumer are linked before the Rust notifications hop. |
| a3 | `a3-async.sh` | Orders producer/consumer branch. | Trace detail: producer span with link to consumer trace. |
| a4 | `a4-reverse.sh` | Java fulfillment produces to Kafka, consumes, then calls Rust notifications. | Trace detail: Java async span link plus Java -> Rust hop. |
| a9 | `a9-field-spike.sh` | Checkout emits baseline logs plus a dominant structured WARN burst. | Logs document fields: `app_screen_name=workspace-select` dominates the spike window. |
| a10 | `a10-baggage.sh` | Checkout sends tenant and tier through HTTP and gRPC W3C baggage. | Traces: checkout, inventory, and pricing have matching `tenant.id` and `user.tier` attributes. |
| a12 | `a12-cli-run.sh` | Short-lived Rust CLI checkout driver. | Runs: command row with exit code; run `cargo build` first. `parallax run start -- scenarios/a12-cli-run.sh` is optional when you want run-scoped resource attrs. |
| a13 | `a13-deploy-regression.sh` | Recreate checkout as `RELEASE=v1`, then `RELEASE=v2` (`A13_BUILD=1` rebuilds images first). | Issues: checkout error spike attributed to `service.version=v2`; release strip lands in plan 041. |
| a14 | `a14-flag-flip.sh` | Flip flagd `paymentFailure` off/on/off without restarting checkout. | Trace detail: `feature_flag.evaluation` events; Issues: failures only while flag is on. |
| a18 | `a18-canary.sh` | Fake sensitive canary corpus in telemetry. | Issues/Logs: redaction of fake email/token/card/jwt fields. |
| a19 | `a19-long-trace.sh` | Checkout emits a synthetic wide/deep `burst.l*` span tree. | Traces: large waterfall stress trace for windowing, minimap, and lane checks. |
| a20-compare | `a20-compare-pair.sh` | Two green checkout variants with structural differences. | Traces: Compare shows added reserve spans, removed recommend branch, and duration deltas. |
| a20 | `a20-batch-fanin.sh` | Orders batch consumer drains rapid publishes into one consumer span. | Trace detail: `consume_batch` has `messaging.batch.message_count=8` and links to each producer trace. |
| a22 | `a22-tokio-saturation.sh` | Checkout `spawn_blocking` flood plus concurrent traffic. | Services -> checkout -> Runtime lane: `tokio.runtime.*` spike; Traces: slow checkout spans in the same window. |
| a23 | `a23-storefront-grpc.sh` | Juniper storefront GraphQL query calls Java payment over gRPC. | Traces: resolver fields followed by `Pricing/Quote` in one trace. |
| a24 | `a24-storefront-catalog.sh` | Juniper storefront GraphQL query calls the Java catalog GraphQL API. | Traces: resolver fields followed by the catalog operation in one trace. |
| a25 | `a25-postgres.sh` | Inventory uses real Postgres for normal reserve, `pg_sleep`, DB-N+1 SELECT fan-out, and pool exhaustion. | Traces: `db.query.text` spans for UPDATE, `pg_sleep`, SELECT fan-out, and `pool_exhausted`; Runtime: `db.client.connection.*` gauges. |
| a26 | `a26-cache.sh` | Recommendation TTL cache cold/warm ratio, bypass, and stampede. | Metrics: `cache_hits_total`, `cache_misses_total`, `cache_size`; Traces: parallel `compute_recommendations` spans; Logs document fields: `cache.hit`. |
| a27 | `a27-execution-stack.sh` | Host CLI to daemon to simulated container and agent/tool spans, plus orphan variant. | Runs/Story: execution beats share one run id; orphan child trace shows `browser_without_backend`. |
| a28 | `a28-rum-journey.sh` | Browser routes, user-step spans, web vitals over OTLP, `session.id`, RUM error, and `nopropagate` broken continuation. | Traces: browser route/user-step spans, `browser.web_vital`, stitched checkout, OTel exception, and disconnected frontend/backend traces for the gap case. |
| a29 | `a29-typed-events.sh` | Typed business log events across Rust, Java, and web tiers. | Logs SQL/Event column: `checkout.completed`, `checkout.failed`, `order.consumed`, `catalog.products.served`, `payment.authorized`, and `web.checkout.submitted`. |
| b-async-chaos | `b-async-chaos.sh` | Consumer lag and poison message. | Services/Traces: lag span and dead-letter error branch. |
| b2 | `b2-inventory-failure.sh` | Inventory returns a deterministic 503 failure. | Traces/Issues: inventory error and checkout impact remain correlated. |
| b5 | `b5-cpu-pressure.sh` | Checkout runs a bounded CPU pressure path. | Services: checkout CPU/runtime saturation appears beside slow request spans. |
| b6 | `b6-cache-leak.sh` | Flip `cacheLeak` and drive recommendation cache growth. | Metrics/Traces: memory growth and the feature-flag evaluation share the window. |
| b10 | `b10-lock-contention.sh` | Concurrent checkout requests contend on one shared lock. | Traces: serialized contention delay is visible across sibling requests. |
| b13 | `b13-slow-recommendation.sh` | Recommendation injects deterministic latency. | Traces: the slow recommendation branch and dependent degradation are visible. |
| b15 | `b15-rage-click.sh` | Playwright repeatedly clicks the unresponsive promo control. | RUM: rage-click and replay evidence attach to the browser journey. |
| b16 | `b16-load.sh` | k6 drives sustained checkout traffic. | Metrics/Traces: sustained request load populates service and trace views. |
| b-chaos | `b-chaos.sh` | Payment failure and injected latency. | Issues/Services: checkout error grouping and slow-span rendering. |
| b-checkout-chaos | `b-checkout-chaos.sh` | Retry timeout and N+1 fan-out. | Traces: retry/timeout branch and N+1 waterfall. |
| b3b | `b3b-grpc-deadline.sh` | Checkout uses tonic `grpc-timeout` against delayed pricing, with retries. | Traces: sibling `pricing.attempt` spans show `rpc.grpc.status_code=4` and `deadline_exceeded` ERROR status. |
| b-degradation | `b-degradation.sh` | Partial degrade and real backdated child span. | Traces/Issues: degraded response and skewed span timing. |
| b17 | `b17-cron.sh` | Short-lived Rust CLI cron mode. | Runs: cron success/fail/stuck outcome; run `cargo build` first. `parallax run start -- scenarios/b17-cron.sh` is optional when you want run-scoped resource attrs. |
| b17b | `b17b-cron-suite.sh` | Cron timeline: ok, ok, fail, stuck, missed, duplicate. | Runs: schedule attrs, exit codes, missing beat, and duplicate `cron.invocation.id`; run `cargo build` first. |
| b19 | `b19-jvm-gc-pressure.sh` | Catalog bounded heap pressure while GraphQL products queries run. | Services -> catalog -> Runtime lane: `jvm.memory.used` / `jvm.gc.*` rise; GraphQL spans slow in the same window. |
| b20 | `b20-container-oom.sh --yes` | Recommendation leak under `deploy/docker-compose.limits.yml` (`mem_limit: 128m`). | Docker OOM/restart evidence plus a recommendation telemetry gap; destructive and requires `--yes`. |
| b21 | `b21-orphan-consumer.sh` | Orders normal linked consumer, orphan linkless consumer, and lag burst. | Traces: normal consumer has a span link, orphan consumer is root/linkless with `messaging.orphan=true`; Runtime: `messaging.queue.depth` rises. |
| b22 | `b22-sampling-gap.sh` | Recreate checkout at `PLAYGROUND_SAMPLE_RATIO=0.1`, drive 50 requests, restore default sampling. | Traces: sampled-out gaps; Logs: full request evidence and dangling trace links. |
| b23 | `b23-uncorrelated-log.sh` | Checkout emits a detached error log outside span context. | Logs: `orphan diagnostic without trace context` row has no trace chip. |
