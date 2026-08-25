# Parallax Telemetry Playground

> This repository is a telemetry-producing workload and verification harness,
> not an observability backend or console.

A maximum-fidelity **OpenTelemetry + Sentry** polyglot sample app — the
comparison *payload* for the [Parallax](https://github.com/tailrocks/parallax)
OTLP fan-out lab. Distinct services in **Rust** and **Java** cross-communicating,
with a **TanStack Start** frontend and a **Rust CLI** driver, instrumented to
exercise every signal so each backend (Parallax, Maple, SigNoz, OpenObserve,
Sentry) can be compared on identical data.

Full design: the Parallax repo's
`docs/research/validation/telemetry-playground-sample-project.md`.
Guided Parallax demo: [`TOUR.md`](TOUR.md). Coverage spine:
[`docs/coverage-matrix.md`](docs/coverage-matrix.md). Display shots:
`artifacts/ui/`.
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

Infra images in `deploy/docker-compose.yml` are pinned 2026-08-14 (plan 162):
`postgres:18`, `redpandadata/redpanda:v26.2.1`, `ghcr.io/open-feature/flagd:v0.16.1`,
`grafana/k6:2.2.0`. Existing `postgres` volumes must be dropped
(`docker compose down -v`) when moving 17→18; schema is created fresh on `up`.

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

**Verified locally (2026-08-14, teaching restamp 21:59Z):** lockstep SDKs = OTel Rust 0.32 +
`tracing-opentelemetry` 0.33 + Sentry Rust 0.49.1 (`sentry-opentelemetry`
adopted for shared `trace_id`); Java agent **2.30.0** + Sentry Spring
**8.53.0** + Boot 4.1.0; OTel JS **2.10 / 0.221** +
`@sentry/tanstackstart-react` 10.70. Fan-out = Rotel `v0.2.5` → OpenObserve
`v0.92.0`, Maple `v0.0.18`, Sentry self-hosted `26.7.2`, host Parallax.
SigNoz omitted (Foundry-only compose).
- Rust workspace: `mise exec -- cargo clippy -D warnings` + nextest **90/90**.
- **OTLP 4-sink live** after `a1` + `b2` + `a6`: OpenObserve
  `checkout=90, catalog=130, payment=76, inventory=19, recommendation=23,
  pricing=5`; Maple `services` API lists the same six names; Parallax
  GraphQL traces include checkout/catalog/payment/inventory and issues
  include Java `IllegalStateException` (GADGET-1) plus Rust inventory chaos.
- `/checkout` still orchestrates pricing (gRPC) + inventory + recommendation
  (HTTP) — `total_minor=3998` at quantity=2.
- **Java agent → Rotel gRPC retested PASS** at agent 2.30.0 / Rotel v0.2.5
  (catalog OO count 56→96 after flipping catalog to `grpc` `:4317` + `a6`).
  Compose now defaults Java to gRPC; HTTP/protobuf `:4318` is the fallback.
- **Sentry** (re-dated 2026-08-14T14:35Z): `verify.sh` A1 OTLP 200; A15/A16
  `PaymentError` `times_seen=10`. Real SDK envelopes land on **both**
  Parallax `/api/1/envelope/` and Sentry Groups: rust `plat=native
  c8-rust-sdk`, java `plat=java c8-java-sdk`, js `plat=node
  Error: c8-js-sdk PaymentError` (`c8 ok rust+java+js`). JS 10.70 first
  POST is `type=session` (Parallax 415); the second POST is `type=event`.
  Compose DSN must stay `host.docker.internal:9000`.
- Java services: upstream OTel agent (never `sentry-opentelemetry-agent`) +
  Spring Sentry starter. Web: `bun run build` + vitest 9/9.

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
