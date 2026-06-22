# Parallax Telemetry Playground

A maximum-fidelity **OpenTelemetry + Sentry** polyglot sample app — the
comparison *payload* for the [Parallax](https://github.com/tailrocks/parallax)
OTLP fan-out lab. Distinct services in **Rust** and **Java** cross-communicating,
with a **TanStack Start** frontend and a **Rust CLI** driver, instrumented to
exercise every signal so each backend (Parallax, Maple, SigNoz, OpenObserve,
Sentry) can be compared on identical data.

Full design: the Parallax repo's
`docs/research/validation/telemetry-playground-sample-project.md`.
Apache-2.0 · Tailrocks.

## Architecture

```
web (TanStack/TS) ─HTTP─► checkout (Rust axum) ─gRPC─► pricing (Rust tonic)
                                  │                ├─gRPC─► payment   (Java Spring gRPC)
                                  │                ├─GraphQL─► catalog (Java Spring GraphQL)
                                  │                ├─HTTP─► inventory / recommendation (Rust)
                                  │                └─publish─► broker ─► fulfillment (Java) ─HTTP─► notifications (Rust)
cli (Rust) ─HTTP─► checkout            flagd (OpenFeature)   loadgen (k6)   Postgres
```

All services export OTLP to the lab's **Rotel** (`OTEL_EXPORTER_OTLP_ENDPOINT`,
default `host.docker.internal:4317`) **and** to Sentry via its SDK (envelope).
One distributed trace stitches browser → Rust → Java → broker → Java → Rust via
W3C trace context.

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
| `services/catalog` | Java Spring GraphQL | ✅ app + schema + Sentry/OTel config — **compiles** (gradlew) |
| `services/payment` | Java Spring | ✅ Spring Boot — **compiles**; gRPC proto codegen is the next step |
| `services/fulfillment` | Java Spring (Kafka) | ✅ consumer + reverse Java→Rust hop — **compiles** |
| `web` | Vite/React (TanStack deps) / TS | ✅ OTel browser provider + Sentry — **builds** (`bun run build`); TanStack router TODO |
| `flags` `loadgen` `scenarios` `deploy` | — | ✅ flagd, k6, scenarios (A1/A3/A12/A18/B1/B11/A13), compose |

**Verified locally (2026-06-23):**
- Rust workspace compiles (`cargo build`, fmt + clippy clean).
- `/checkout` orchestrates **pricing (gRPC) + inventory (HTTP) + recommendation
  (HTTP)** in one request — real multi-service distributed trace, `otel.kind`
  server/client spans, correct aggregated response (HTTP 200).
- **Chaos** verified: `?fail=1`→502 error issue (B1), `?slow=ms`→latency (B11).
- **Canary** verified: `?canary=1` plants a redaction corpus (email/token/card/
  jwt) in span attrs + log body (A18).
- **Async branch** verified: orders PRODUCER→CONSUMER spans with a span LINK
  carrying the producer's trace_id (A3).
- All three Java services compile (`gradlew compileJava`).
- web builds (`bun run build`, 642 modules → dist).

## Run

```bash
# Rust core (no Docker): two terminals
cargo run --bin pricing
PRICING_ENDPOINT=http://localhost:50051 cargo run --bin checkout
curl "http://localhost:8088/checkout?sku=WIDGET-1&quantity=3"

# Everything, against the fan-out lab:
#   1. start the lab (parallax repo: bench/otlp-fanout) so Rotel is on :4317
#   2. docker compose -f deploy/docker-compose.yml up --build
#   3. scenarios/a1-checkout.sh
```

## Roadmap

Java (catalog/payment/fulfillment) + web wiring, then the async/broker, chaos
(flagd), deploy-regression, and canary-redaction scenarios — per the design doc's
phasing. Comparison is manual (open each backend's UI); a scored harness is out
of scope for now.
