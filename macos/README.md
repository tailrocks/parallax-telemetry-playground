# macOS telemetry playground

Deterministic native-macOS telemetry harness: a dependency-free Swift CLI that
emits hand-encoded OTLP/HTTP protobuf (Parallax accepts protobuf only) for two
scenarios, plus a real-crash dSYM symbolication proof. No Linux-container
fakery: everything here executes on macOS against macOS APIs.

## Layout

- `Package.swift`, `Sources/`, `Tests/` — Swift harness (Foundation + os +
  MetricKit only; builds offline with system Swift).
- `tools/stub_receiver.py` — dependency-free OTLP/HTTP stub (ephemeral port).
- `tools/verify.py` — full verification: build, unit tests, determinism,
  stub wire + correlation proof, unified-log roundtrip, crash + `atos`/dSYM
  proof, optional live Parallax leg (`--parallax`).

## Scenarios

| Mode | Span | Logs | Metrics | Join key |
|---|---|---|---|---|
| `failure` | `macos.checkout.submit` CLIENT, ERROR + `exception` event with real `Thread.callStackSymbols` stack | ERROR + INFO lifecycle, same trace/span | `macos.playground.failures` +1 | one trace_id |
| `slow-op` | `macos.report.render` INTERNAL, 2.4s | WARN hang-suspected | `macos.playground.operation.duration` histogram, sample in (1000,5000]ms bucket | one trace_id |
| `crash` | — (a crashed process cannot report itself) | — | — | `.ips` report + dSYM |

`--parent-traceparent 00-<trace>-<span>-01` joins a backend trace as a child
span in both the failure and slow-op scenarios (covered by
`testParentTraceparentJoinsBackendTrace` and
`testSlowOpParentTraceparentJoinsBackendTrace`).

Resource attributes on every signal: `service.name/version/instance`,
`os.type=darwin`, `os.description`, `host.arch`, `device.model.identifier`
(sysctl `hw.model`), `macos.thermal_state`, `macos.scenario_id`.

## Run

```bash
swift build
.build/debug/MacOSPlayground all --seed 42 --dry-run | head -c 600
.build/debug/MacOSPlayground failure --seed 42 --endpoint http://127.0.0.1:4318
```

Flags: `--seed`, `--frozen-time <unix_nanos>` (byte-determinism),
`--endpoint`, `--parent-traceparent`, `--service`, `--scenario-id`,
`--unified-log` (also emits `os_log`, verifiable via `log show`),
`--metrickit-probe-seconds N`.

## Verify

```bash
python3 tools/verify.py              # no backend needed (25 checks)
python3 tools/verify.py --parallax   # + live Parallax ingest/query leg
```

The `--parallax` leg probes OTLP/API pairs (stock `4318/4000`, isolated
`14328/14000`) and skips token-walled instances. Only one *managed* Parallax
fits per host (engine ports 24000–24003 are hardcoded), so for the isolated
leg start a second engine + Parallax in `external` mode first:

```bash
greptime standalone start --http-addr 127.0.0.1:24100 --rpc-bind-addr 127.0.0.1:24101 \
  --mysql-addr 127.0.0.1:24102 --postgres-addr 127.0.0.1:24103 \
  --data-home /tmp/macotel-parallax/greptime-data &
parallax serve --config /tmp/macotel-parallax/config.toml &  # [storage] mode="external",
                                                            # greptime_url="http://127.0.0.1:24100",
                                                            # loopback bind, api_port=14000, otlp_http_port=14328
```

## Verified vs blocked (2026-09-13, macOS 26.6.2 arm64, Swift 6.3.3)

Verified (see `tools/verify.py`, 25/25 green):

- OTLP protobuf accepted by Parallax (`traces`/`logs`/`metrics` → 200).
- One trace_id joins span + logs + metrics per scenario (stub parse + Parallax
  `trace`/`logsByTrace`/`traceEvents`).
- Parallax derives `CheckoutDeclined` issues pointing at the macOS trace.
- Real `fatalError` crash → `.ips` report → `atos` + dSYM DWARF resolves
  `static Harness.main() (Harness.swift:58)`; dSYM UUID matches binary.
- `os_log` roundtrip via `log show`; W3C child-join; deterministic IDs.

Honestly blocked / out of scope:

- **MetricKit payloads**: subscribing from a CLI yields zero payloads
  (proven: `--metrickit-probe-seconds 5` → 0/0). Real `MXCrashDiagnostic` /
  `MXHangDiagnostic` delivery needs an installed GUI app identity plus
  Apple's aggregation delay. Mapping code exists upstream
  (`opentelemetry-swift` MetricKit instrumentation); Parallax-side ingestion
  of `callStackTree` + server symbolication is specified in
  `parallax/docs/research/macos/` but not implemented here.
- **Server-side dSYM symbolication in Parallax**: no upload/symbolicate path
  exists in Parallax today; the harness proves the client/half (crash,
  dSYM build, UUID match, `atos` resolution).
- **SwiftUI lifecycle/hang auto-instrumentation**: needs a real `.app` host;
  the harness emits equivalent lifecycle/hang telemetry manually.
