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
| `services/inventory` `recommendation` `notifications` | Rust | ✅ telemetry-wired scaffolds — **build** |
| `cli` | Rust | ✅ run driver — **builds** |
| `services/catalog` | Java Spring GraphQL | 🟡 scaffold (Application + schema + Sentry/OTel config) |
| `services/payment` `fulfillment` | Java Spring | 🟡 scaffold (README + build plan) |
| `web` | TanStack Start / TS | 🟡 scaffold (Sentry init + deps; provider wiring TODO) |
| `flags` `loadgen` `scenarios` `deploy` | — | ✅ flagd config, k6 load, A1/A12 drivers, compose |

**Verified locally:** the Rust workspace compiles (`cargo build`, 0 warnings) and
the checkout→pricing distributed call returns correctly (`total_minor` = unit ×
qty), with `otel.kind` server/client spans on both sides.

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
