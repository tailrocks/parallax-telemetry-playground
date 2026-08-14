# Coverage matrix

Spine for the playground verification program. Every Parallax inventory
line and every TELEMETRY COMPLETENESS concept has a row. Empty cells and
`UNTESTED` are forbidden. A proven SDK impossibility is `DISPOSITION` with
an upstream link.

- Semconv citations: OpenTelemetry Semantic Conventions **1.37.0** registry
  (Rust crate `opentelemetry-semantic-conventions` 0.32.1; JS
  `@opentelemetry/semantic-conventions` 1.43.0). Playground frozen wire
  names live in `docs/semconv-registry-inventory.md`.
- Status vocabulary: `MISSING` (no scripted scenario yet), `MAPPED` (scenario
  exists; live dated PASS/FAIL still required), `PASS` / `FAIL` (live run),
  `DISPOSITION` (cannot emit; upstream cited).
- Date: 2026-08-14. Live cells dated from c-series + a30/a31 + agent-browser.

## Completeness — traces

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Browser fetch → SSR `traceparent` meta → Rust axum HTTP | web TanStack + checkout axum | a28, a1 | `traceparent`, `http.request.method`, `http.route`, `url.path` | Traces waterfall | MAPPED |
| tonic gRPC unary | checkout → pricing | a1 | `rpc.system=grpc`, `rpc.service`, `rpc.method` | Traces | MAPPED |
| tonic gRPC server-streaming + `rpc.message` + fail + cancel | checkout → pricing | a7b | `rpc.message`, `messaging` n/a, stream events | Traces RPC streams | MAPPED |
| Java Spring GraphQL DataLoader vs N+1 + partial errors + op-name policy | catalog GraphQL | a6 | `graphql.operation.name`, field spans | Traces GraphQL ops | MAPPED |
| Spring gRPC Rust→Java | checkout → payment | a23 | `rpc.system=grpc`, `rpc.service=playground.pricing.v1.Pricing` | Traces | MAPPED |
| Kafka/Redpanda producer/consumer span links | orders + fulfillment | a3, a8, a4 | `messaging.system`, `messaging.operation`, span links | Trace detail links | MAPPED |
| Batch fan-in many links | orders | a20 | `messaging.batch.message_count` | Trace detail | MAPPED |
| Poison → dead-letter | orders / fulfillment | b-async-chaos, b21 | `messaging.destination.name`, error status | Traces / Issues | MAPPED |
| Reverse Java→Rust hop | fulfillment → notifications | a4 | `http.request.method` client + server | Traces | MAPPED |
| All five span kinds | mixed | a1 (server/client), a3 (producer/consumer), a25 (internal db) | `otel.kind` | Traces color-by | MAPPED |
| Exception span events + stacktrace encoding | checkout, catalog, web | t-events, a5, b2 | `exception.type`, `exception.message`, `exception.stacktrace` | Trace events / Issues | MAPPED |
| Span links beyond messaging (batch aggregation) | orders | a20 | span links | Trace links | MAPPED |
| Span status OK vs ERROR vs UNSET | pricing / checkout | p-grpc-err, b3b | `otel.status_code`, `rpc.grpc.status_code` | Traces | MAPPED |
| W3C baggage tenant/tier ≥3 services | checkout, inventory, pricing | a10 | `tenant.id`, `user.tier` baggage | Trace attributes | MAPPED |
| Long/wide traces | checkout synthetic | a19, t-wide, t-deep | span tree | Waterfall virtualization | MAPPED |
| Broken propagation (`nopropagate`) | web → checkout | a28 | `telemetry.propagation.disabled` | Two disconnected traces | MAPPED |
| Clock-skew demonstration | synthetic | t-skew, b-degradation | start/end timestamps | Clock-skew banner | MAPPED |
| Async fire-and-forget vs awaited | orders | a3 | producer vs server child | Trace compare | MAPPED |
| db spans `db.query.text` (sqlx/Postgres: param, pg_sleep, N+1, pool) | inventory | a25 | `db.system.name`, `db.query.text`, `db.operation.name`, `db.client.connection.*` | Traces + Runtime | MAPPED |
| Cache hit/miss/stampede | recommendation | a26 | `cache.hit`, cache metrics | Metrics + Traces | MAPPED |
| Retry storms + gRPC deadline | checkout → pricing | b3b, b-checkout-chaos | `rpc.grpc.status_code=4` | Traces | MAPPED |
| Feature-flag evaluation events | checkout + catalog flagd | a14 | `feature_flag.evaluation` events | Trace events | MAPPED |
| GraphQL-over-WebSocket subscription | catalog | a7 | subscription span | Traces | MAPPED |
| Storefront GraphQL → catalog | storefront Juniper | a24 | graphql + http | Traces | MAPPED |

## Completeness — metrics

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Counter | catalog Micrometer + checkout RED | a2, a1 | `catalog.product.queries`, `http.server.request.duration` | Metrics | MAPPED |
| Up-down counter | checkout + rust HTTP middleware | a30 | `http.server.active_requests` | Metrics | PASS 2026-08-14 (`http_server_active_requests` in metricNames) |
| Gauge | tokio + db pool + cache size | a22, a25, a26 | `tokio.runtime.*`, `db.client.connection.*`, `cache_size` | Services Runtime | MAPPED |
| Explicit-bucket histogram | checkout RED | a1 | `http.server.request.duration` | Metrics workbench | PASS 2026-08-14 (workbench `/metrics/http_server_request_duration_seconds`) |
| Exponential histogram (JVM W5 probe) | catalog agent | a2 + compose `OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION` | exp histogram | Metrics + VERIFICATION W5 | MAPPED |
| Summaries | — | — | OTel JS/Rust/Java SDKs do not emit OTLP summaries | Metrics | DISPOSITION — OTel dropped Summary as a first-class OTLP metric type; no SDK emit path. See [OTEP 203](https://github.com/open-telemetry/oteps/blob/main/text/0203-more-metrics-data-model.md) / [spec metrics data model](https://opentelemetry.io/docs/specs/otel/metrics/data-model/). |
| Exemplars (JVM `trace_based`) | catalog | a2 | exemplar `trace_id` | Metrics → trace | MAPPED |
| Exemplars (Rust) | Rust SDK | — | — | Metrics | DISPOSITION — Rust SDK has no exemplars; [opentelemetry-rust#3369](https://github.com/open-telemetry/opentelemetry-rust/issues/3369) |
| JVM runtime GC/memory/threads | catalog agent | b19 | `jvm.memory.used`, `jvm.gc.*` | Services Runtime | MAPPED |
| Tokio + process metrics | checkout | a22 | `tokio.runtime.*` | Services Runtime | MAPPED |
| RED-derivable request metrics | checkout | a1 | `http.server.request.duration` + status | Services RED | MAPPED |
| Bounded high-cardinality teaching label | checkout | a30 | `playground.cardinality.events` + `demo.bucket` ∈ 0..15 | Metrics | PASS 2026-08-14 (`playground_cardinality_events_total`) |
| Cumulative vs delta temporality noted per exporter | all OTLP exporters | VERIFICATION.md §temporality | default CUMULATIVE; `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE=delta` for delta | docs | MAPPED |

## Completeness — logs

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Severity ladder | checkout + catalog logback | a9, l-bodies | `severity_text`, `severity_number` | Logs | MAPPED |
| Structured KV (logback MDC / tracing fields / browser) | catalog, checkout, web | a9, a29 | MDC + tracing fields | Logs columns | MAPPED |
| Trace-correlated logs per tier | all | a1, a29 | `trace_id`, `span_id` on log records | Logs trace chip | MAPPED |
| Log field-cardinality spike | checkout | a9 | `app.screen.name` | Logs facets | MAPPED |
| Typed business events shared vocab | rust/java/web | a29 | `event.name` = `checkout.completed` etc. | Logs Event column | MAPPED |
| Multiline / stacktrace logs | t-events | t-events | exception stacktrace | Logs / Issues | MAPPED |
| Uncorrelated log stream | checkout | b23 | no trace context | Logs without chip | MAPPED |

## Completeness — errors + Sentry dual path

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Exception span events | rust/java/web | t-events, b2, a5 | `exception.*` | Issues | MAPPED |
| Log-based ERROR events | checkout | b23, b2 | severity ERROR | Issues | MAPPED |
| Sentry envelopes Rust | checkout | b2 | Sentry SDK 0.49 | Sentry project + Parallax envelope ingest | MAPPED |
| Sentry envelopes Java (Spring starter, **not** sentry-otel agent) | catalog/payment | a14 | sentry-spring-boot-4-starter 8.53 | Sentry + Issues | MAPPED |
| Sentry envelopes browser JS | web | a5 | `@sentry/tanstackstart-react` 10.70 | Sentry + Issues | MAPPED |
| Cross-language same-error grouping (`PaymentError`) | rust/java/browser | e-multi-lang | `error.type` | Issues grouping | MAPPED |
| Release/deploy regression v1→v2 | checkout | a13 | `service.version`, `vcs.ref.head.revision` | Issues + Services release strip | MAPPED |
| Handled vs unhandled | checkout | a31 | handled `PaymentError` 502 vs unhandled panic 500 | Issues | PASS 2026-08-14 (502 vs empty-reply 000) |
| Browser RUM + `session.id` + web-vitals + rage-click | web | a28, a5, b15 | `session.id`, `browser.web_vital`, `ui.click` | Traces / CLI Apps / Issues | MAPPED |

## Completeness — resource + correlation + load

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| Full required resource set every service | all | a1 + compose `OTEL_RESOURCE_ATTRIBUTES` | `service.name`, `service.version`, `deployment.environment.name`, `vcs.ref.head.revision`, `cli.invocation.id` | Services / Traces resource | MAPPED |
| `cli.invocation.id` from CLI driver into tests | playground-cli + gradle/nextest | a12, a27, observable-test-session | `cli.invocation.id`, `TRACEPARENT` | CLI Apps / Tests | MAPPED |
| Test-report bridge JUnit + flaky fail-then-pass | rust/java/web tests | test-verify | `test.case.*` | Tests explorer | MAPPED |
| GitHub deploy/CI webhook fixtures same vcs | — | c6 | `vcs.ref.head.revision` | Services deploy | PASS 2026-08-14 (c6 good=200 bad=401) |
| k6 ambient 24h mix | loadgen | b16 | RED + traces | Overview / Metrics | MAPPED |
| Burst + chaos via flagd | flagd | a-breach-*, a14, a-recover | feature flags | Alerts / Issues | MAPPED |

## Completeness — Parallax product surfaces (c-series)

| Concept | Services / tech | Scenario | Semconv | Parallax surface | Status |
|---|---|---|---|---|---|
| c1 issue-context / evidence bundle | checkout errors | c1 | issue fingerprint + bundle hash | Issues + `bundle` GraphQL | PASS 2026-08-14 |
| c2 invocation lifecycle | playground-cli | c2 | `cli.invocation.id` | CLI Apps hub | PASS 2026-08-14 |
| c3 live tail | checkout logs/spans | c3 | SSE `/v1/logs/stream` `/v1/traces/stream` | Logs/Traces live | PASS 2026-08-14 |
| c4 alerting end-to-end + webhook | checkout | c4 | error_rate / p95 | Alerts incidents | PASS 2026-08-14 |
| c5 saved state (dashboard/investigation/view/SQL) | GraphQL mutations | c5 | n/a metadata | Dashboards / Investigations / Logs views / SQL | PASS 2026-08-14 |
| c6 GitHub webhooks | fixtures | c6 | deploy + Actions | Services deploy / Tests CI | PASS 2026-08-14 |
| c7 agent-session import + MCP | `import-claude` + parallax-mcp | c7 | agent spans | Story / MCP | PASS 2026-08-14 |
| c8 Sentry-envelope parity per SDK | rust/java/web | c8 | envelope ingest | Issues from `/api/1/envelope/` | PASS 2026-08-14 |
| c9 lifecycle ops isolated-HOME prune | parallax CLI | c9 | n/a | doctor / prune | PASS 2026-08-14 (doctor+prune; not self-OTLP) |
| c10 redaction-egress canary | a18 corpus on every egress | c10 | canary.* | Issues/Logs/bundle/webhook | PASS 2026-08-14 |
| c11 agent-browser UI pass | all seeded surfaces | c11 | n/a | 21-route checklist | PASS 2026-08-14 (c11 smoke + full walk `artifacts/ui/`) |

## Inventory — ingest / storage / CLI / API / UI / ops

Rows below map shipped Parallax features from
`docs/research/reference/feature-inventory-and-playground-verification.md`
(parallax, 2026-08-13). Telemetry-facing rows reuse completeness scenarios;
product-surface rows wait on c-series.

| Inventory line | Scenario | Parallax surface | Status |
|---|---|---|---|
| OTLP/gRPC `:4317` + OTLP/HTTP `:4318` traces/logs/metrics | a1 | Ingest / Traces / Logs / Metrics | PASS 2026-08-14 (scratch OTLP 14317/14318) |
| Sentry envelope `POST /api/<project>/envelope/` | c8 | Ingest / Issues | PASS 2026-08-14 |
| GitHub webhooks deploy + Actions | c6 | Deploy / Tests | PASS 2026-08-14 |
| Claude Code `import-claude` | c7 | Story / agent session | PASS 2026-08-14 |
| Raw-frame spool → workers → Greptime → issues → live | a1 | `/health`, live SSE | PASS 2026-08-14 |
| Greptime native tables + Turso metadata | a1 (implicit) | Storage | MAPPED (implicit in every live query) |
| `parallax prune` pin-aware | c9 | prune CLI | PASS 2026-08-14 (dry-run; never `--execute` on real HOME) |
| Deterministic error derivation + fingerprint grouping | e-burst, e-multi-lang | Issues | PASS 2026-08-14 (Issues UI + c1) |
| Evidence bundles + `missing_evidence` + pins | c1 | Issues handoff / GraphQL `bundle` | PASS 2026-08-14 |
| Redaction-lite-v3 20 detectors | a18, c10 | all egress | PASS 2026-08-14 (a18 + c10 bundle canary) |
| Story timeline + agent-session + fixer outcomes | c7, a27 | Story | PASS 2026-08-14 (c7 import) / MAPPED (a27) |
| MCP `parallax_issue_context` + `parallax_agent_session_show` | c7 | MCP stdio | PASS 2026-08-14 (c7 import path; MCP show is the same session id) |
| CLI `serve`/`doctor`/`sql`/`metrics` | c9 | CLI | PASS 2026-08-14 (doctor) / MAPPED (`sql` UI) |
| CLI `logs`/`traces` `--follow --for` | c3 | CLI live | PASS 2026-08-14 (SSE bytes; UI Query→Live) |
| CLI invocations `start/finish/inspect/bundle` | c2, a12 | CLI Apps | PASS 2026-08-14 |
| CLI `issue list/context/resolve` | c1 | Issues CLI | PASS 2026-08-14 |
| GraphQL 76q/14m families listed in inventory | c1–c5 | GraphQL | PASS 2026-08-14 (c1–c5 machine asserts) |
| SSE live tail | c3 | Logs/Traces live | PASS 2026-08-14 |
| UI Overview / Issues / Traces / Logs / Metrics / Services / Ecosystem / CLI Apps / Tests / Alerts / Dashboards / Investigations / SQL | c11 + a/b | each route | PASS 2026-08-14 (agent-browser walk; Tests page empty of cases) |
| Alerting rules + incidents + webhook/Slack | c4, a-breach-* | Alerts | PASS 2026-08-14 (c4 incident) |
| Test reporting JUnit/nextest/flaky | test-verify | Tests | MAPPED (explorer empty on this host; page PASS) |
| Self-telemetry `PARALLAX_SELF_OTLP` | c9 | ingest of parallax itself | MAPPED — c9 is doctor/prune only; self-OTLP not asserted |
| Profiles / GraphQL subscriptions / SLO / alert email | — | — | DISPOSITION — product gaps, not playground emit gaps (inventory "Known gaps") |

## Gap list

Emit gaps closed by `a30` / `a31` and the temporality note in
`VERIFICATION.md`. Inventory `MISSING` cells from the first cut are now
dated `PASS 2026-08-14` from `c1`–`c11` (retry log) + the agent-browser
walk (`artifacts/ui/`). Still honest, not PASS:

- Tests explorer had zero `testCases` on this host (page renders).
- `PARALLAX_SELF_OTLP` is not asserted by `c9`.
- SDK/product `DISPOSITION` rows (summaries, Rust exemplars, profiles/SLO
  /alert email) stay cited, not blank.
