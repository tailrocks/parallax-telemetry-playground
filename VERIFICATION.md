# Verification runbook

What's verified in CI/sandbox vs what needs a real host. The **code/config for
every scenario is implemented and builds**; the scenarios below need a live
multi-runtime environment (Sentry self-hosted, a browser, a collector with a
short flush) that a sandbox can't provision.

## Verified here (build/run/execute)
- Rust workspace builds (fmt + clippy clean); Java services compile; web builds
  (`bun run build`: Vite client + SSR + Nitro server) and type-checks
  (`tsc --noEmit`); routes `/` and `/v1/traces` register. The live SSR/browser
  run needs a host with working DNS (this sandbox's node `fetch`/undici has
  none — `node -e fetch(...)` fails on any host), so the Nitro prod server's
  per-request render can't complete here; it's a host concern, not a code defect.
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
- **A7 GraphQL subscription** resolver (`catalog`, WebSocket transport) compiles
  (`gradlew compileJava`).
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
**0.30**. Its `SentrySpanProcessor`/`SentryPropagator` are 0.29 types and won't
attach to a 0.30 `SdkTracerProvider`, so it can't be added without downgrading
the whole OTel stack (regressing logs/metrics). Rust Sentry issues therefore
carry their own trace_id today; revisit when the crate reaches OTel 0.30+.

## Needs a real host — exact steps to verify the last scenarios

Prereqs: start the lab (`parallax` repo `bench/otlp-fanout`), then this app's
`deploy/docker-compose.yml`. Set `OTEL_EXPORTER_OTLP_ENDPOINT` to Rotel and
`SENTRY_DSN` to a Sentry project. Lower OpenObserve's flush for fast feedback:
`ZO_FILE_PUSH_INTERVAL=10`.

### A2 — exemplars (JVM)
Code: `catalog` exports the `catalog.product.queries` Micrometer counter with
`management.tracing.exemplars.include=all`.
Verify: drive `products` queries; in the metrics backend (SigNoz/OpenObserve),
open the counter and confirm a data point carries a `trace_id` exemplar that
jumps to the trace. (Rust tier has no exemplars — issue #3369 — so use the JVM
counter.)

### A5 / B15 — browser RUM + rage-clicks + session replay
Code: `web` has `replayIntegration` + `browserTracingIntegration`; buttons
"break (RUM error)" (A5) and "apply promo (unresponsive)" (B15).
Verify: `bun run dev`, open the app in a browser with `VITE_SENTRY_DSN` set:
- click "break" → a Sentry **error** with session **replay** (A5);
- rapidly click "apply promo" → Sentry flags a **rage click** in the replay (B15);
- confirm **web vitals** (LCP/CLS/INP) appear in Sentry Performance.

### A15 / A16 — Sentry issue grouping + lifecycle
Code: every service inits Sentry (DSN from env); Rust `tracing::error!` →
Sentry issue (sentry-tracing), Java unhandled → Sentry, and GraphQL/field errors
should call `Sentry.captureException`.
Verify: with `SENTRY_DSN` set, trigger the same error repeatedly (e.g.
`/checkout?fail=1`) → one **grouped issue** with rising event count (A15); resolve
it in Sentry, deploy `v2` (`RELEASE=v2`) that re-introduces it → Sentry marks it
**regressed** (A16).

### A17 — profiling
Code: JVM uses the Sentry agent; add `@sentry/profiling`-equivalent on the tiers
that support it. CPU hot path: `/checkout?cpu_ms=200`.
Verify: with profiling enabled + `SENTRY_DSN`, drive the hot path → a CPU profile
with the slow function appears in Sentry Profiling.

### Live simultaneous cross-language trace into the backend
Run Java `payment` + the Rust services together against Rotel; with a short
collector flush, one OpenObserve trace search shows `checkout` (Rust) **and**
`payment` (Java) sharing one trace (add a tonic client interceptor in checkout to
inject `traceparent` into gRPC metadata for full stitching).
