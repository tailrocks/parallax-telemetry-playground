# Parallax Telemetry Playground

A maximum-fidelity **OpenTelemetry + Sentry** polyglot sample app — the
comparison *payload* for the [Parallax](https://github.com/tailrocks/parallax)
OTLP fan-out lab. Distinct services in **Rust** and **Java** cross-communicating,
with a **TanStack Start** frontend and a **Rust CLI** driver, instrumented to
exercise every signal so each backend (Parallax, Maple, SigNoz, OpenObserve,
Sentry) can be compared on identical data.

Full design: the Parallax repo's
`docs/research/validation/telemetry-playground-sample-project.md`.
Guided Parallax demo: [`TOUR.md`](TOUR.md).
Apache-2.0 · Tailrocks.

## TypeScript policy

All tracked web, server, configuration, and k6 load-generator source uses
ordinary `.ts`/`.tsx`. Tracked `.js`, `.jsx`, `.mjs`, `.cjs`, `.mts`, and
`.cts` source/configuration is forbidden. The single web compiler project
checks the application, Bun production server, and both k6 programs with
`strict: true`, `allowJs: false`, `checkJs: false`, and the repository's
additional strict flags; `scripts/check-typescript-policy.sh` fails closed on
file or configuration drift.

## Architecture

```
web (TanStack/TS) ─HTTP─► checkout (Rust axum) ─gRPC─► pricing (Rust tonic)
                                  │                ├─gRPC─► payment   (Java Spring gRPC)
                                  │                ├─GraphQL─► catalog (Java Spring GraphQL)
                                  │                ├─HTTP─► inventory / recommendation (Rust)
                                  │                └─publish─► broker ─► fulfillment (Java) ─HTTP─► notifications (Rust)
cli (Rust) ─HTTP─► checkout            flagd (OpenFeature)   loadgen (k6, demo profile)   Postgres (catalog + inventory)
```

All services export OTLP to a host listener on `4317`/`4318`: local
`parallax serve` or the fan-out lab's **Rotel**. They also export to Sentry
via SDK/envelope paths. One distributed trace stitches browser -> Rust -> Java
-> broker -> Java -> Rust via W3C trace context.

## Status

| Component | Lang | State |
|---|---|---|
| `libs/playground-telemetry` | Rust | ✅ OTel traces + tracing + Sentry init — **builds** |
| `proto` | Rust | ✅ pricing gRPC contract — **builds** |
| `services/checkout` | Rust axum | ✅ HTTP→gRPC orchestrator — **builds + runs** (verified) |
| `services/pricing` | Rust tonic | ✅ gRPC server — **builds + runs** (verified) |
| `services/inventory` `recommendation` | Rust | ✅ HTTP services in the checkout trace — **build + run** (verified) |
| `services/orders` | Rust | ✅ async branch: producer/consumer spans + **span link** — **builds + runs** (verified) |
| `services/storefront` | Rust Juniper / Axum | ✅ GraphQL→catalog and GraphQL→gRPC gateway with WebSocket subscriptions — **builds** |
| `services/notifications` | Rust | ✅ reverse-hop target — **builds** |
| `cli` | Rust | ✅ run driver — **builds** |
| `services/catalog` | Java Spring GraphQL | ✅ **A6 DataLoader** (`@BatchMapping`) + **A14 OpenFeature/flagd** flag eval + Postgres/JDBC path — GraphQL slice and JUnit tests pass locally |
| `services/payment` | Java Spring **gRPC** | ✅ real Spring gRPC server from the shared proto — Rust→Java gRPC verified; in-process transport and JUnit tests pass locally |
| `services/fulfillment` | Java Spring (Kafka) | ✅ **real Kafka producer + consumer** round-trip + reverse Java→Rust hop — consumer handoff and JUnit tests pass locally |
| `web` | TanStack Start / TS | ✅ real TanStack Start app (file routing + Nitro): same-origin `/v1/traces` OTLP proxy, SSR `<meta traceparent>`, OTel browser + Sentry RUM — **builds + type-checks** (`bun run build`) |
| `flags` `loadgen` `scenarios` `deploy` | — | ✅ flagd, k6, scenarios, compose (all services incl. Java + web; `Dockerfile.java`/`Dockerfile.web`) |

## Test-telemetry conventions

The checked-in generated semantic-convention files are the sole source for test
run telemetry across the playground. Do not hand-copy these wire names:

| Stack | Generated source |
|---|---|
| Rust | `libs/playground-telemetry/src/semconv.rs` |
| Java | `services/semconv/src/main/java/io/tailrocks/semconv/Semconv.java` |
| Web | `web/src/semconv.ts` |

The shared test payload uses `test.case.name`, `test.case.result.status`,
`test.suite.name`, `test.suite.run.status`, `cicd.pipeline.run.id`,
`cicd.pipeline.task.type`, and `parallax.test.id` when an explicit stable test
identity is available. Regenerate them only from Parallax with
`cargo xtask semconv --playground-root ../parallax-telemetry-playground generate`.

The acceptance run is executable and machine-checked against Parallax rather
than accepted from screenshots:

```bash
parallax invocation start -- scripts/observable-test-session.sh web --acceptance
mise exec -- cargo run --locked -p playground-cli -- \
  test-verify <run-id-printed-above> web
```

Use `rust`, `java`, or `web` consistently in both commands. The verifier polls
the GraphQL API for the finished run and fails unless it finds the exported
run-session parent, complete identity/configuration/retry/failure payload,
assertion and harness failures, version/revision resources, and application
spans descended from a test span.

**Verified locally (2026-06-23):**
- Rust workspace compiles (`cargo build`, fmt + clippy clean).
- **Integrated end-to-end**: the four Rust services emit OTLP → the fan-out lab's
  **Rotel** → **OpenObserve**; a trace search returns all four services
  (`checkout=25, pricing=5, inventory=5, recommendation=5` spans). This is the
  whole pipeline working, not just stdout.
- `/checkout` orchestrates **pricing (gRPC) + inventory + recommendation (HTTP)**
  in one request — `otel.kind` server/client spans, correct aggregation.
- **A7 streaming**: gRPC server-streaming (`/quote-stream?quantity=4` → 4 quotes).
- **A3 async**: orders PRODUCER→CONSUMER with a span LINK to the producer.
- **A10 baggage**, **A18 canary** corpus in span/log.
- **Chaos verified**: B1 fail→502, B2 inventory 503, B3 retry/timeout, B5 high-CPU,
  B6 cache-leak, B7 consumer-lag, B8 poison→dead-letter, B9 N+1, B10 lock
  contention, B11 latency, B17 cron (success/fail/stuck).
- Java services use the upstream OTel agent for fan-out plus the Spring Sentry
  SDK for envelopes; web builds and runs with Bun (`bun run build`, `bun start`).
- **Cross-language gRPC verified**: Rust `checkout` (tonic client) → **Java
  `payment`** (Spring gRPC server, Boot 4.1 + Spring gRPC 1.1, generated from the
  shared proto) returns the Java-computed price (`3998`); the OTel Java agent
  produces a proper `playground.pricing.v1.Pricing/Quote` SERVER span (rpc
  semconv). *(Note: the Java agent's OTLP→Rotel→OpenObserve delivery has an
  environment-specific snag still being chased — the Rust path into OpenObserve is
  verified; Java instrumentation is verified via the logging exporter.)*

## Run

```bash
# Demo against Parallax (primary)
# 1. In the Parallax repo:
parallax serve

# 2. In this repo:
./demo.sh

# 3. Drive one story and open http://localhost:4000:
scenarios/run.sh a1

# Fan-out lab comparison (kept working)
# 1. Start the lab (parallax repo: bench/otlp-fanout) so Rotel is on :4317
# 2. docker compose -f deploy/docker-compose.yml up --build
# 3. scenarios/a1-checkout.sh
```

CLI scenarios need the Rust binary first:

```bash
cargo build
./target/debug/playground
./target/debug/playground cron
# Convert cargo-nextest's JUnit XML into run-parented test telemetry.
./target/debug/playground test-report target/nextest/ci/junit.xml
```

Generate that durable report locally with the same profile used by the test
telemetry bridge:

```bash
cargo nextest run --workspace --profile ci --no-tests=fail
./target/debug/playground test-report target/nextest/ci/junit.xml
```

## Roadmap

Java (catalog/payment/fulfillment) + web wiring, then the async/broker, chaos
(flagd), deploy-regression, and canary-redaction scenarios — per the design doc's
phasing. Comparison is manual (open each backend's UI); a scored harness is out
of scope for now.

## Corner-case corpus

`docs/corner-case-matrix.md` maps every UI rendering risk to a stable
scenario id (`scenarios/run.sh <id>`); run the whole corpus with
`scenarios/corner-cases.sh --all-corner-cases`. Synthetic shapes export as
`service.name=playground-shapes`; journey cases ride `playground console`.
