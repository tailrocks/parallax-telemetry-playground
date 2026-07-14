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

## Architecture

```
web (TanStack/TS) ─HTTP─► checkout (Rust axum) ─gRPC─► pricing (Rust tonic)
                                  │                ├─gRPC─► payment   (Java Spring gRPC)
                                  │                ├─GraphQL─► catalog (Java Spring GraphQL)
                                  │                ├─HTTP─► inventory / recommendation (Rust)
                                  │                └─publish─► broker ─► fulfillment (Java) ─HTTP─► notifications (Rust)
cli (Rust) ─HTTP─► checkout            flagd (OpenFeature)   loadgen (k6, demo profile)   Postgres (reserved; DB scenarios later)
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
| `services/notifications` | Rust | ✅ reverse-hop target — **builds** |
| `cli` | Rust | ✅ run driver — **builds** |
| `services/catalog` | Java Spring GraphQL | ✅ **A6 DataLoader** (`@BatchMapping`) + **A14 OpenFeature/flagd** flag eval — **compiles** |
| `services/payment` | Java Spring **gRPC** | ✅ real Spring gRPC server from the shared proto — **compiles + runs**; Rust→Java gRPC verified |
| `services/fulfillment` | Java Spring (Kafka) | ✅ **real Kafka producer + consumer** round-trip + reverse Java→Rust hop — **compiles** |
| `web` | TanStack Start / TS | ✅ real TanStack Start app (file routing + Nitro): same-origin `/v1/traces` OTLP proxy, SSR `<meta traceparent>`, OTel browser + Sentry RUM — **builds + type-checks** (`bun run build`) |
| `flags` `loadgen` `scenarios` `deploy` | — | ✅ flagd, k6, scenarios, compose (all services incl. Java + web; `Dockerfile.java`/`Dockerfile.web`) |

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
  SDK for envelopes; web builds with Bun (`bun run build`).
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
```

## Roadmap

Java (catalog/payment/fulfillment) + web wiring, then the async/broker, chaos
(flagd), deploy-regression, and canary-redaction scenarios — per the design doc's
phasing. Comparison is manual (open each backend's UI); a scored harness is out
of scope for now.
