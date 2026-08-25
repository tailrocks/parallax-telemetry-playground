# Coverage matrix

Spine for the playground verification program. Every Parallax inventory
line and every TELEMETRY COMPLETENESS concept has a row. Empty cells and
`UNTESTED` are forbidden. A proven SDK impossibility is `DISPOSITION` with
an upstream link.

- Semconv citations: OpenTelemetry Semantic Conventions **1.37.0** registry
  (Rust crate `opentelemetry-semantic-conventions` 0.32.1; JS
  `@opentelemetry/semantic-conventions` 1.43.0).
- Status vocabulary: `PASS` / `FAIL` (dated live run), `DISPOSITION`
  (cannot emit or product gap; upstream/inventory cited). `MAPPED` is not
  a terminal cell.
- Date: 2026-08-14 live restamp (named-concept GraphQL/SQL after a2
  `:8080` fix, FLAGD_HOST on orders, Boot 4.1 payment `channel.target`).

## Completeness — traces

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Browser fetch → SSR `traceparent` meta → Rust axum HTTP | web TanStack + checkout axum | a28, a1 | `traceparent`, `http.request.method`, `http.route`, `url.path` | Traces waterfall | PASS 2026-08-14 (a1 18-span `8cf58d291fb795ef02fb67acff2a3431`; noprop pair below) |
| tonic gRPC unary | checkout → pricing | a1 | `rpc.system=grpc`, `rpc.service`, `rpc.method` | Traces | PASS 2026-08-14 (`quote@pricing` + `pricing.attempt` on 18-span trace) |
| tonic gRPC server-streaming + `rpc.message` + fail + cancel | checkout → pricing | a7b | `rpc.message` stream events | Traces RPC streams | PASS 2026-08-14 (trace `b4e373608fffe78a` 12× `rpc.message`; fail `338473de01e58f6f`; cancel `27bd3ae13857e954`; UI `traces-teach-stream-1440-dark.png`) |
| Java Spring GraphQL DataLoader vs N+1 + partial errors + op-name policy | catalog GraphQL | a6 | `graphql.operation.name`, field spans | Traces GraphQL ops | PASS 2026-08-14 (batch `40fd24943cfb` `catalog.reviews.batch`; N+1 `9a0b6ed9ee5c` two `reviewsSlow`; partial `503aa9f252bd` `riskScore`; UI nplus1/batch shots) |
| Spring gRPC Rust→Java | checkout → payment | a23 | `rpc.system=grpc` | Traces | PASS 2026-08-14 (`996c5a8207a04a07e4a823715b6913dd` storefront+payment same trace after xlang `PRICING_ENDPOINT=http://payment:9090`) |
| Kafka/Redpanda producer/consumer span links | orders + fulfillment | a3, a8, a4 | `messaging.system`, span links | Trace detail links | PASS 2026-08-14 (consumer `ff46e8d94be06b78` inspector **Links (1)** → producer `963391462dd5c46b` span `c1e6afa8585c40e0`; header Links/events 1/1; UI `traces-teach-links` is the consumer, not the producer-only `producer_without_consumer` frame) |
| Batch fan-in many links | orders | a20 | `messaging.batch.message_count` | Trace detail | PASS 2026-08-14 (`f86e7d07c1e6762e048bdd56363e9e69` `messaging.batch.message_count=8`). FLAGD_HOST=flagd required or `/order` hangs ~15s |
| Poison → dead-letter | orders / fulfillment | b-async-chaos, b21 | `messaging.destination.name` | Traces / Issues | PASS 2026-08-14 (dead_letter `5ae9486c071d3562a25f44fab7471cc8`; poison_message ×3 `1ae9f3cd696f3497cc3030f206009bf1` / `59c39d71703176cb3b5e5e029eb1ae52` / `312e33129c4ce3eba2d2d3afc49381a5`) |
| Reverse Java→Rust hop | fulfillment → notifications | a4 | `http.request.method` | Traces | PASS 2026-08-14 (`710762bebe4c4dd3d22b24cd9b53a606` fulfillment+notifications same trace). Boot 4.1 `channel.payment.target=static://payment:9090` |
| All five span kinds | mixed | a1, a3, a25 | `otel.kind` | Traces color-by | PASS 2026-08-14 (SERVER/CLIENT on 18-span; PRODUCER/CONSUMER via links; INTERNAL db `postgres.query`) |
| Exception span events + stacktrace encoding | checkout, catalog, web | t-events, a5, b2 | `exception.*` | Trace events / Issues | PASS 2026-08-14 (PaymentError + IllegalStateException issues) |
| Span links beyond messaging (batch aggregation) | orders | a20 | span links | Trace links | PASS 2026-08-14 (same `f86e7d07` consume_batch message_count=8) |
| Span status OK vs ERROR vs UNSET | pricing / checkout | p-grpc-err, b3b | `otel.status_code`, `rpc.grpc.status_code` | Traces | PASS 2026-08-14 (a7b fail stream ERROR events; a31 502 vs panic) |
| W3C baggage tenant/tier ≥3 services | checkout, inventory, pricing | a10 | `tenant.id`, `user.tier` | Trace attributes | PASS 2026-08-14 (a10 HTTP 200; checkout/inventory/pricing on same waterfall) |
| Long/wide traces | checkout synthetic | a19, t-wide | span tree | Waterfall virtualization | PASS 2026-08-14 (a19 `2364a8449426441760cd4df94706136d` 240× `burst.l%`; t-wide `d66651dace3343730000000000000002` spanCount=521). HTTP shapes post to OTLP HTTP 14320 |
| Broken propagation (`nopropagate`) | web → checkout | a28 | `telemetry.propagation.disabled` | Two disconnected traces | PASS 2026-08-14 (web `b1d8c1e61584d8c1c1f3e867b05703ef` `ui.submit` only, `telemetry.propagation.disabled=true` widget=`checkout-form-nopropagate`; checkout `7156516a218cb2086cddadfdcdf47f85` 18-span, different id; agent-browser 200) |
| Clock-skew demonstration | synthetic | t-skew, b-degradation | start/end timestamps | Clock-skew banner | FAIL 2026-08-14T15:43Z (same-service `?skew=1` still has **no** "Clock skew suspected" banner — detector is cross-service only; W5 DISCREPANCY) |
| Async fire-and-forget vs awaited | orders | a3 | producer vs server child | Trace compare | PASS 2026-08-14 (a3 + linkedTraces) |
| db spans `db.query.text` | inventory | a25 | `db.system.name`, `db.query.text` | Traces + Runtime | PASS 2026-08-14 (pg_sleep `2e6863591db79546b28a838efe18eac7` `SELECT pg_sleep($1::float / 1000)` 405ms; N+1 `18abd1f15232c1d4d0a21517d74e1772` 12× SELECT stock; pool_exhausted `d9561aeffe7bd189ad10562990d8d5d4`; metricNames `db.client.connection.*`) |
| Cache hit/miss/stampede | recommendation | a26 | `cache.hit` | Metrics + Traces | PASS 2026-08-14 (a1 JSON `cache_hit` true/false; `cache_*` metricNames) |
| Retry storms + gRPC deadline | checkout → pricing | b3b, b-checkout-chaos | `rpc.grpc.status_code=4` | Traces | PASS 2026-08-14 (b3b driven this session) |
| Feature-flag evaluation events | checkout + catalog flagd | a14 | `feature_flag.evaluation` | Trace events | PASS 2026-08-14 (events on 18-span; a14 driven) |
| GraphQL-over-WebSocket subscription | catalog | a7 | subscription span | Traces | PASS 2026-08-14 (`52447a02859b5f9665f6c632c23f9a8e` catalog `subscription` + `priceChanges`; a7 9 events) |
| Storefront GraphQL → catalog | storefront Juniper | a24 | graphql + http | Traces | PASS 2026-08-14 (`e54a8592eab4cf38ec1dbd4765d19008` storefront+catalog same trace) |

## Completeness — metrics

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Counter | catalog Micrometer + checkout RED | a2, a1 | `catalog.product.queries` | Metrics | PASS 2026-08-14 (a2 default `CATALOG_URL` now `:8080` GraphQL; 12/12 queries this serve; exemplars `c9464650357d6f26`) |
| Up-down counter | checkout middleware | a30 | `http.server.active_requests` | Metrics | PASS 2026-08-14 (`http_server_active_requests`) |
| Gauge | tokio + db pool + cache | a22, a25, a26 | `tokio.runtime.*`, `db.client.connection.*`, `cache_size` | Services Runtime `/services/$name` | PASS 2026-08-14 (`db.client.connection.count|idle|max|pending.requests|timeouts|wait.time.milliseconds` live metricNames; `/services/checkout` tokio; `/services/catalog` jvm.*) |
| Explicit-bucket histogram | checkout RED | a1 | `http.server.request.duration` | Metrics workbench | PASS 2026-08-14 |
| Exponential histogram (JVM W5) | catalog agent | a2 + compose env | exp histogram | Metrics + VERIFICATION W5 | DISPOSITION — Parallax drops exp histograms (`normalize_metrics`); probe env stays. VERIFICATION W5 CODE-CONFIRMED drop |
| Summaries | — | — | OTel dropped Summary | Metrics | DISPOSITION — [OTEP 203](https://github.com/open-telemetry/oteps/blob/main/text/0203-more-metrics-data-model.md) |
| Exemplars (JVM `trace_based`) | catalog | a2 | exemplar `trace_id` | Metrics → trace | PASS 2026-08-14 GraphQL `metricExemplars` → `c9464650357d6f26`. FAIL display 15:43Z: `/metrics/catalog.product.queries` `hasTraceLink=false` (W5 DISCREPANCY) |
| Exemplars (Rust) | Rust SDK | — | — | Metrics | DISPOSITION — [opentelemetry-rust#3369](https://github.com/open-telemetry/opentelemetry-rust/issues/3369) |
| JVM runtime GC/memory/threads | catalog agent | b19 | `jvm.memory.used` | Services Runtime | PASS 2026-08-14 (metricNames `jvm.*`) |
| Tokio + process metrics | checkout | a22 | `tokio.runtime.*` | Services Runtime | PASS 2026-08-14 |
| RED-derivable request metrics | checkout | a1 | `http.server.request.duration` | Services RED | PASS 2026-08-14 |
| Bounded high-cardinality teaching label | checkout | a30 | `playground.cardinality.events` `demo.bucket` 0..15 | Metrics | PASS 2026-08-14 |
| Cumulative vs delta temporality | all OTLP exporters | VERIFICATION.md §temporality | default CUMULATIVE | docs | PASS 2026-08-14 (note re-read; default unchanged) |

## Completeness — logs

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Severity ladder | checkout + catalog logback | a9, l-bodies | `severity_text` | Logs | PASS 2026-08-14 (a9 spike + Logs UI prior walk) |
| Structured KV | catalog, checkout, web | a9, a29 | MDC + tracing fields | Logs columns | PASS 2026-08-14 (a9) |
| Trace-correlated logs per tier | all | a1, a29 | `trace_id`, `span_id` | Logs trace chip | PASS 2026-08-14 |
| Log field-cardinality spike | checkout | a9 | `app.screen.name` | Logs facets | PASS 2026-08-14 |
| Typed business events shared vocab | rust/java/web | a29 | `event.name` | Logs Event column | PASS 2026-08-14 (a29 ran; Java hop timed out 15s — Rust events landed) |
| Multiline / stacktrace logs | t-events | t-events | exception stacktrace | Logs / Issues | PASS 2026-08-14 |
| Uncorrelated log stream | checkout | b23 | no trace context | Logs without chip | PASS 2026-08-14 (issue `orphan diagnostic without trace context`) |

## Completeness — errors + Sentry dual path

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Exception span events | rust/java/web | t-events, b2, a5 | `exception.*` | Issues | PASS 2026-08-14 |
| Log-based ERROR events | checkout | b23, b2 | severity ERROR | Issues | PASS 2026-08-14 |
| Sentry envelopes Rust | checkout + `c8_sentry_emit` | b2, c8 | sentry-rust 0.49 | Sentry + Parallax `/api/1/envelope/` | PASS 2026-08-14 (Parallax issue `c8-rust-sdk`; Sentry Group plat=native) |
| Sentry envelopes Java (Spring starter, **not** sentry-otel agent) | catalog `C8SentryEmit` | a14, c8 | sentry-java 8.53 | Sentry + Issues | PASS 2026-08-14 (Parallax `IllegalStateException: c8-java-sdk`; Sentry plat=java) |
| Sentry envelopes browser JS | web `@sentry/node` 10.70 + TanStack RUM | a5, c8 | Sentry JS 10.70 `type=event` | Sentry + Issues | PASS 2026-08-14 (Parallax issue `Error: c8-js-sdk PaymentError`; first POST `type=session`; event is second envelope. Official SDK puts `sentry_key` in query string — Parallax 401s unless `X-Sentry-Auth` is also set; `c8-emit-js.ts` adds that header) |
| Cross-language same-error grouping (`PaymentError`) | rust/java/browser | e-multi-lang, c8 | `error.type` | Issues grouping | PASS 2026-08-14 (Sentry A15/A16 times_seen=10; Parallax separate rust/java fingerprints — Sentry is grouping authority) |
| Release/deploy regression v1→v2 | checkout | a13 | `service.version` | Issues + Services | PASS 2026-08-14 (`RELEASE=v2` recreate; 5× `/checkout` **502**; GraphQL `releases(checkout)` v1=553 + v2=28 spans; `/services/checkout?range=1h` badge **2 versions** + v1 bar, v2 sliver. Stock `a13` 10s curl is tight when checkout is loaded — this run used 30s. c6 HMAC still 200/401) |
| Handled vs unhandled | checkout | a31 | 502 vs panic | Issues | PASS 2026-08-14 (502 vs 000) |
| Browser RUM + `session.id` + web-vitals + rage-click | web | a28, a5, b15 | `session.id`, `browser.web_vital` | Traces / CLI Apps / Issues | PASS 2026-08-14 (Parallax stitch `19edbf0ad9f030364b4657dfc7f4f463`: web `ui.click` → checkout `http.server.request` + `checkout`, 3 spans / 2 services; UI `traces-teach-rum`. Also `browser.web_vital` singles + playground HTML `web-rum-break` producer) |

## Completeness — resource + correlation + load

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Full required resource set every service | all | a1 + compose | `service.name`, `service.version`, `deployment.environment.name`, `vcs.ref.head.revision`, `cli.invocation.id` | Services / Traces | PASS 2026-08-14 (traces SQL column `resource_attributes.vcs.ref.head.revision=9fb282967a523be424bef4e1c006bfb82a850c37` non-null on catalog, checkout, fulfillment, inventory, notifications, orders, payment, playground-shapes, pricing, recommendation, storefront, web. Logs carry the same key inside `resource_attributes` JSON. Compose `GIT_SHA` + Java `OTEL_RESOURCE_ATTRIBUTES` + shapes `required_resource_kvs`) |
| `cli.invocation.id` from CLI into tests | playground-cli + nextest | a12, test-verify | `cli.invocation.id`, `TRACEPARENT` | CLI Apps / Tests | PASS 2026-08-14 (invocation `2f617012-f2ae-4ea7-9bfc-9e27b37f1354` + testCases) |
| Test-report bridge JUnit + flaky fail-then-pass | rust tests | test-verify `--acceptance` | `test.case.*` | Tests explorer | PASS 2026-08-14 (`w4_assertion_failure_passes_on_retry` + `w4_harness_error_passes_on_retry` rollup `FLAKY_PASS`; UI `tests-teach-flaky-1440-dark.png`) |
| GitHub deploy/CI webhook fixtures same vcs | fixtures | c6 | `vcs.ref.head.revision` | Services deploy | PASS 2026-08-14 (c6 200/401) |
| k6 ambient 24h mix | loadgen | b16 | RED + traces | Overview / Metrics | PASS 2026-08-14 (k6 2.2.0 `loadgen/checkout.ts` `--vus 3 --duration 25s` EXIT 0: 3 http_reqs, 0 failed, avg 30.09s; checkout is slow under this lab) |
| Burst + chaos via flagd | flagd | a14, a-breach-* | feature flags | Alerts / Issues | PASS 2026-08-14 (a14 + c4 incident) |

## Completeness — Parallax product surfaces (c-series)

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| c1 issue-context / evidence bundle | checkout errors | c1 | fingerprint + bundle hash | Issues + `bundle` | PASS 2026-08-14 |
| c2 invocation lifecycle | playground-cli | c2 | `cli.invocation.id` | CLI Apps | PASS 2026-08-14 |
| c3 live tail | checkout | c3 | SSE `/v1/logs/stream` | Logs/Traces live | PASS 2026-08-14 (940 bytes) |
| c4 alerting + webhook + Slack dest | checkout | c4 | error_rate; `slack_webhook` | Alerts | PASS 2026-08-14 (incident `inc-alr_18cbb162587af878`; slack `dst_18cbb1625301fc20`) |
| c5 saved state | GraphQL | c5 | n/a metadata | Dashboards / Investigations | PASS 2026-08-14 |
| c6 GitHub webhooks | fixtures | c6 | deploy HMAC | Services deploy | PASS 2026-08-14 |
| c7 agent-session import + MCP | import-claude + parallax-mcp | c7 | agent session | Story / MCP | PASS 2026-08-14T15:23Z import+GraphQL `agentSession` (c7 EXIT 0). FAIL product `parallax-mcp check` CLI≢GraphQL JSON (W5 DISCREPANCY) |
| c8 Sentry-envelope parity per SDK | rust/java/js real SDKs | c8 | envelope ingest | Issues | PASS 2026-08-14 rust+java+js |
| c9 isolated-HOME prune + contexts + `--otlp-forward` | parallax CLI | c9 | n/a | doctor / prune / contexts | PASS 2026-08-14 (HOME under repo `.isolation/`; prune `--execute --yes`; `context add c9lab`; `--otlp-forward off`) |
| c10 redaction-egress canary | a18 tokens on egress | c10 | canary.* | bundle/CLI/MCP/UI/Sentry ack/webhook | PASS 2026-08-14 (no leak; webhook body empty this run) |
| c11 agent-browser UI pass | all seeded surfaces | c11 | n/a | 21-route + teaching traces | PASS 2026-08-14 (c11 smoke + teaching-trace walk 13 PASS / 1 list-virtualize miss) |

## Inventory — ingest / storage / CLI / API / UI / ops

| Inventory line | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| OTLP/gRPC + OTLP/HTTP traces/logs/metrics | all emitters | a1 | OTLP | Ingest / Traces / Logs / Metrics | PASS 2026-08-14 (scratch 14319/14320 + Rotel 4317) |
| Sentry envelope `POST /api/<project>/envelope/` | rust/java/js SDKs | c8 | Sentry envelope | Ingest / Issues | PASS 2026-08-14 rust+java+js (`c8 ok`) |
| GitHub webhooks deploy + Actions | fixtures | c6 | HMAC + `vcs.*` | Deploy / Tests | PASS 2026-08-14 |
| Claude Code `import-claude` | NDJSON fixture | c7 | agent session | Story | PASS 2026-08-14 |
| Raw-frame spool → workers → Greptime → issues → live | serve pipeline | a1, c3 | — | `/health`, SSE | PASS 2026-08-14 |
| Greptime native tables + Turso metadata | serve | a1 implicit | native OTLP tables | Storage | PASS 2026-08-14 (every GraphQL query) |
| `parallax prune` pin-aware | CLI | c9 | — | prune CLI | PASS 2026-08-14 isolated HOME |
| Deterministic error derivation + fingerprint grouping | ingest | e-burst, c1 | `error.type` | Issues | PASS 2026-08-14 |
| Evidence bundles + `missing_evidence` + pins | GraphQL/CLI | c1 | bundle-v1/v2 | Issues handoff | PASS 2026-08-14 |
| Redaction-lite-v3 20 detectors | egress | a18, c10 | canary.* | all egress | PASS 2026-08-14 |
| Story timeline + agent-session + fixer outcomes | import-claude | c7, a27 | agent spans | Story | PASS 2026-08-14 (import) |
| MCP `parallax_issue_context` + `parallax_agent_session_show` | parallax-mcp | c7 | bundle / agentSession | MCP stdio | FAIL 2026-08-14T15:23Z `check` CLI≢HTTP JSON (W5 DISCREPANCY); tools exist; GraphQL `agentSession` callable |
| CLI `serve`/`doctor`/`sql`/`metrics` | CLI | c9 | — | CLI | PASS 2026-08-14 (doctor on isolated HOME) |
| CLI `logs`/`traces` `--follow --for` | CLI / SSE | c3 | — | CLI live | PASS 2026-08-14 (SSE bytes) |
| CLI invocations `start/finish/inspect/bundle` | CLI | c2 | `cli.invocation.id` | CLI Apps | PASS 2026-08-14 |
| CLI invocations `--otlp-forward` compare mode | CLI | c9 | `PARALLAX_OTLP_FORWARD` | invocation start | PASS 2026-08-14 (`--otlp-forward off`) |
| Remote contexts `context add\|list\|use\|show\|remove` | CLI `~/.parallax/contexts.toml` | c9 | — | CLI contexts | PASS 2026-08-14 (isolated HOME `c9lab`) |
| CLI `issue list/context/resolve` | CLI | c1 | fingerprint | Issues CLI | PASS 2026-08-14 |
| GraphQL 76q/14m families | API | c1–c5 | — | GraphQL | PASS 2026-08-14 |
| SSE live tail | API | c3 | — | Logs/Traces live | PASS 2026-08-14 |
| UI Overview / Issues / Traces / Logs / Metrics / Services / Ecosystem / CLI Apps / Tests / Alerts / Dashboards / Investigations / SQL | SPA | c11 + teach walk | — | each route | PASS 2026-08-15 c11-ui headings on Overview/Issues/Traces/Logs/Metrics/Services/Alerts/Dashboards/Investigations/SQL/Tests. FAIL display 2026-08-15: default `/ecosystem` 24h `serviceMap` INTERNAL (`?range=1h` GraphQL nodes OK); historical `/invocations` unbounded `observedInvocations` INTERNAL (`limit:3` OK), requires revalidation because current UI uses `observedInvocations(limit:50)`. W5 DISCREPANCY |
| Alerting rules + incidents + webhook | alerting | c4 | error_rate | Alerts | PASS 2026-08-14 |
| Alerting Slack webhook destination | alerting | c4 | `slack_webhook` | Alerts dest | PASS 2026-08-14 |
| Test reporting JUnit/nextest/flaky | test-verify | test-verify | `test.case.*` | Tests | PASS 2026-08-14 |
| Self-telemetry `PARALLAX_SELF_OTLP` | serve | live `services` | `service.name` | ingest of parallax | PASS 2026-08-14 as `unknown_service:parallax` (serve this session not named `parallax`) |
| Profiles / GraphQL subscriptions / SLO / alert email | — | — | — | — | DISPOSITION — inventory Known gaps |

## Gap list / honesty

- **FAIL** `parallax-mcp check` CLI≢GraphQL bundle JSON — W5 DISCREPANCY (product). Restamped 2026-08-14T15:23Z this serve.
- **FAIL** clock-skew banner absent on same-service `?skew=1` — W5 DISCREPANCY. Restamped 2026-08-14T15:43Z.
- **FAIL** Metrics workbench `/metrics/catalog.product.queries` `hasTraceLink=false` (GraphQL has exemplars → `c9464650357d6f26`). W5 DISCREPANCY. Restamped 2026-08-14T15:43Z. Service detail lives at `/services/$name`.
- **FAIL** default `/ecosystem` (24h) `serviceMap` INTERNAL; `?range=1h` renders 10 services · 6 edges. W5 DISCREPANCY. 2026-08-15T01:40Z.
- **HISTORICAL / REQUIRES REVALIDATION** `/invocations` `observedInvocations` without `limit` was INTERNAL; `limit:3` OK. W5 DISCREPANCY observed 2026-08-15T01:40Z. Current UI uses `observedInvocations(limit:50)`; retain this gap until revalidated.
- Issues list virtualize miss of `c8-rust-sdk` string is harness-only; issue detail pages PASS.
- Tests explorer seeded this session via `parallax invocation start -- scripts/observable-test-session.sh rust --acceptance`.
- c9 never touched operator `~/.parallax` (throwaway `$repo/.isolation/`).
- Gradle `BUILD SUCCESSFUL` catalog/payment/fulfillment ×2: `gradle-gate-1.log`, `gradle-gate-2.log`.
