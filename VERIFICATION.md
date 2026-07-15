# Verification runbook

For the guided Parallax demo path see [`TOUR.md`](TOUR.md); for the quick
traffic generator see `./demo.sh`. This file remains the full cross-backend
verification runbook.

What's verified in CI/sandbox vs what needs a real host. The **code/config for
every scenario is implemented**; Rust, web build/Vitest, the three Java test
suites, and Playwright browser journeys run locally. Java uses a temporary
Gradle cache outside the container's native library mount; the scenarios below
still need a live multi-runtime environment (Sentry self-hosted and a collector
with a short flush) that this sandbox cannot provision.

## Verified here (build/run/execute)
- Rust workspace builds (fmt + clippy clean); web builds (`bun run build`: Vite
  client + SSR + Nitro server) and type-checks (`tsc --noEmit`); routes `/` and
  `/v1/traces` register. Catalog, payment, and fulfillment clean Gradle suites
  pass on this Linux arm64 host with `GRADLE_USER_HOME=/tmp/parallax-gradle`
  and `-Dorg.gradle.native=false`; this avoids the home-mounted native cache
  that cannot load Gradle/Jansi libraries.
- Chromium browser execution is locally proven on 2026-07-15. The arm64
  sandbox lacks system browser libraries, so the test command uses a
  user-owned extracted runtime and Fontconfig configuration outside the
  repository: the default Playwright suite passes five journeys with two W4
  retry fixtures skipped, and the W4 opt-in run intentionally records one
  assertion failure and one harness timeout before both pass on retry. This
  proves the browser UI contracts and failure taxonomy; collector-backed trace
  inspection remains a live-stack gate.
- The complete Rust nextest `ci` profile passes 57 tests across 11 binaries;
  its generated JUnit XML is accepted by `playground test-report` as 57 passed
  cases with no implicit localhost exporter.
- `parallax run start` compare-mode forward (Parallax repo, 11 tests).
- Lab fan-out: trace → Rotel → OpenObserve (queried back by service).
- Multi-service Rust distributed trace (checkout → pricing/inventory/recommendation).
- **Cross-language gRPC**: Rust checkout → Java payment (Spring gRPC) returns the
  Java-computed price; Java OTel agent emits the `Pricing/Quote` SERVER span.
- Scenarios A1, A3, A6, A7, A8, A9, A10, A12, A13, A14, A18 and B1–B13, B16–B18;
  real Kafka producer/consumer round-trip.
- **Live multi-service trace re-verified on the upgraded deps (2026-06-23):**
  the four Rust services (checkout/pricing/inventory/recommendation, now on
  otel 0.32 / tonic 0.14) run against a live Dockerized lab (Rotel +
  OpenObserve); `/checkout` drives gRPC + HTTP fan-out, and an OpenObserve trace
  search returns `checkout=30, pricing=6, inventory=6, recommendation=6` spans.
  (The OO search path is `/api/{org}/_search` with the stream in the SQL FROM +
  from/size — the lab's `smoke.sh` was fixed to match.)
- **Rust tier emits all three OTLP signals** (traces + metrics + logs) — was
  traces-only; `cargo build` + fmt + clippy clean.
- **A7 GraphQL subscription** resolver (`catalog`, WebSocket transport) is
  source-covered; catalog's GraphQL slice runs locally.
- **A17 profiling** wired on the JVM services (Sentry continuous profiling
  config); flamegraph view needs a live Sentry (below).
- **Sentry envelope emit path verified (2026-06-23)** — ran `checkout` with
  `SENTRY_DSN` pointed at a mock receiver and triggered the B1 error
  (`/checkout?fail=1`). The mock captured real Sentry envelopes: a `type:"event"`
  issue (`message:"payment failure (chaos)", logger:"checkout"`, A15/A16 emit)
  **and** a `type:"transaction"` performance envelope
  (`transaction:"checkout", release:"0.1.0", environment:"playground", sdk:
  sentry.rust 0.48.2`). So the playground's Sentry path — errors → issues, spans
  → transactions, correct release/env metadata — emits correctly. Issue
  *grouping/lifecycle rendering* (A15/A16) and the *flamegraph view* (A17) are
  Sentry-server product behavior, viewed in a live Sentry UI (the deferred
  ~72-service self-hosted stack — see below).

### Known version blocker — Rust `sentry-opentelemetry` (shared trace_id)
`sentry-opentelemetry` 0.48 pins `opentelemetry` **0.29**; the workspace is on
**0.32**. Its `SentrySpanProcessor`/`SentryPropagator` are 0.29 types and won't
attach to a 0.32 `SdkTracerProvider`, so it can't be added without downgrading
the whole OTel stack (regressing logs/metrics). Rust Sentry issues therefore
carry their own trace_id today; revisit when the crate reaches OTel 0.30+.

## Needs a real host — exact steps to verify the last scenarios

Prereqs: start the lab (`parallax` repo `bench/otlp-fanout`), then this app's
`deploy/docker-compose.yml`. Set `OTEL_EXPORTER_OTLP_ENDPOINT` to Rotel and
`SENTRY_DSN` to a Sentry project. Lower OpenObserve's flush for fast feedback:
`ZO_FILE_PUSH_INTERVAL=10`.

### A2 — exemplars (JVM)
Code: `catalog` exports the `catalog.product.queries` Micrometer counter with
the Java agent exemplar filter set by `OTEL_METRICS_EXEMPLAR_FILTER=trace_based`
in compose.
Verify: drive `products` queries; query GreptimeDB's native metric table for
the counter and inspect exemplar columns/metadata for a `trace_id` that links
to the trace. (Rust tier has no exemplars — issue #3369 — so use the JVM
counter.)

### W5 — exponential-histogram conformance probe (JVM)

Code: the `catalog` Java-agent service sets
`OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION=base2_exponential_bucket_histogram`.
Drive catalog GraphQL/HTTP traffic after the fan-out stack starts, then inspect
the exported histogram's aggregation shape rather than assuming a backend
conversion is lossless.

| Backend | Expected recording evidence | Result |
| --- | --- | --- |
| Parallax | Native ingest disposition (currently expected to drop unsupported exponential histograms) | pending live run |
| Maple | Histogram type/buckets visible or documented conversion | pending live run |
| SigNoz | Histogram type/buckets visible or documented conversion | pending live run |
| OpenObserve | Histogram type/buckets visible or documented conversion | pending live run |
| Sentry | Metrics rendering/disposition recorded | pending live run |

### W4 — Java test telemetry

Code: catalog, payment, and fulfillment attach the pinned upstream
`opentelemetry-javaagent` to every Gradle `Test` JVM. The existing
OpenTelemetry Gradle plugin supplies task/per-test spans and forwards the run
identity plus parent context; the agent instruments integration-test client
work beneath those test executions. JUnit XML keeps `mergeReruns=true` as the
authoritative retry record.

Verify: run each service's Gradle tests with `TRACEPARENT`, `PARALLAX_RUN_ID`,
and `OTEL_EXPORTER_OTLP_ENDPOINT` set, then inspect the test root, failure
payload, and any HTTP/gRPC/Kafka/JDBC child spans in the same trace. With no
endpoint, local tests explicitly set the Java agent's trace/metric/log
exporters to `none`, preventing false connection-refused errors; a supplied
endpoint preserves the live-export path. On this host use
`GRADLE_USER_HOME=/tmp/parallax-gradle` plus `-Dorg.gradle.native=false` to
avoid the home-mounted native cache.

### W4 — Rust in-process test telemetry

Code: setting `PLAYGROUND_TEST_TELEMETRY=1` activates the shared Rust helper
inside the real notifications loopback test. It extracts the supplied W3C
`TRACEPARENT`, installs the test dispatcher and parent context as one scope,
then explicitly shuts down the simple OTLP exporter after the test body.
Default nextest runs remain exporter-free; the JUnit converter remains the
complete result bridge for every Rust test.

The shared deterministic unit fixture separately proves that this scope makes
the propagated remote trace ID the active OpenTelemetry parent, without a
collector or network dependency.

Verify: set `PLAYGROUND_TEST_TELEMETRY=1`, `TRACEPARENT`, and
`OTEL_EXPORTER_OTLP_ENDPOINT` for a focused notifications nextest run. Inspect
the test-run parent plus the HTTP server/client work below it in the collector.

### W3 — inventory Postgres reservation

Code: inventory's opt-in integration test uses the production SQLx pool and
reservation route against a compose-provided Postgres URL. It seeds an isolated
SKU, asserts the real atomic decrement, removes that row, and closes the pool.

Verify: `INVENTORY_TEST_DATABASE_URL=postgres://... cargo nextest run -p
inventory reserves_stock_against_an_opt_in_postgres_database`. Without that
explicit variable, the test prints a skip diagnostic and leaves the normal
Docker-free suite unchanged.

### W5 — cross-language `PaymentError` grouping

Code: Rust checkout's B1 failure and Java payment's `Quote` request with
`sku=PAYMENT-ERROR` both record `error.type=PaymentError` and the exact message
`PaymentError: payment failed` before their HTTP/gRPC transport layers render a
failure. Java also sends the original exception to Sentry.

Verify: trigger both paths against the same fan-out window. Record whether
Sentry groups them (expected), whether Parallax's fingerprint associates them,
and whether Maple, SigNoz, and OpenObserve retain only their trace/log error
attributes. Do not call a shared `error.type` alone evidence of product-level
grouping; record the rendered backend result in this section.

The runnable driver is `CROSS_LANGUAGE_PAYMENT_ERROR=1
scenarios/b-chaos.sh` after starting the existing
`deploy/docker-compose.xlang.yml` overlay, which routes checkout's pricing call
to Java payment.

### A5 / B15 — browser RUM + rage-clicks + session replay
Code: `web` has `replayIntegration` + `browserTracingIntegration`; buttons
"break (RUM error)" (A5) and "apply promo (unresponsive)" (B15).
Verify: `bun run dev`, open the app in a browser with `VITE_SENTRY_DSN` set:
- click "break" → a Sentry **error** with session **replay** (A5);
- rapidly click "apply promo" → Sentry flags a **rage click** in the replay (B15);
- confirm **web vitals** (LCP/CLS/INP) appear in Sentry Performance.

### A15 / A16 — Sentry issue grouping + lifecycle
Code: every service initializes Sentry from `SENTRY_DSN`; Rust `tracing::error!`
emits a Sentry issue through `sentry-tracing`, and Java uses the Spring SDK
starter alongside the upstream OTel agent. GraphQL/field errors should call
`Sentry.captureException` when they are handled rather than allowed to escape.
Verify: with `SENTRY_DSN` set, trigger the same error repeatedly (e.g.
`/checkout?fail=1`) → one **grouped issue** with rising event count (A15); resolve
it in Sentry, deploy `v2` (`RELEASE=v2`) that re-introduces it → Sentry marks it
**regressed** (A16).

### A17 — profiling
Code: JVM uses the Sentry Spring SDK's continuous-profiling configuration while
the upstream OTel agent remains the OTLP exporter. CPU hot path:
`/checkout?cpu_ms=200`.
Verify: with profiling enabled + `SENTRY_DSN`, drive the hot path → a CPU profile
with the slow function appears in Sentry Profiling.

### Live simultaneous cross-language trace into the backend
Run Java `payment` + the Rust services together against Rotel; with a short
collector flush, one OpenObserve trace search shows `checkout` (Rust) **and**
`payment` (Java) sharing one trace (add a tonic client interceptor in checkout to
inject `traceparent` into gRPC metadata for full stitching).
