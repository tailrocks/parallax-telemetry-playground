# macOS telemetry playground

Deterministic native-macOS telemetry harness: a dependency-free Swift CLI that
emits hand-encoded OTLP/HTTP protobuf (Parallax accepts protobuf only) so a
developer can correlate a native failure or slow operation with backend, logs,
traces, metrics, and release on **one trace_id**. No Linux-container fakery.

## Layout

- `Package.swift`, `Sources/`, `Tests/` — Swift harness (Foundation + os +
  MachO + MetricKit; builds offline with system Swift).
- `tools/stub_receiver.py` — dependency-free OTLP/HTTP stub (ephemeral port).
- `tools/verify.py` — full verification: build, unit tests, determinism,
  stub wire + correlation proof, URLSession W3C inject, unified-log, unsigned
  `.app` release context, MetricKit honest-zero, crash + `atos`/dSYM proof,
  opportunistic live Parallax (`--parallax` or auto if a listener is up).

## Scenarios

| Mode | Spans | Logs | Metrics | Join key |
|---|---|---|---|---|
| `failure` | `macos.checkout.submit` INTERNAL (ERROR + `exception` + real stack) + `HTTP POST` CLIENT child with injected `traceparent` | ERROR (stack) + INFO lifecycle | `macos.playground.failures` + duration histogram, **exemplars carry trace_id** | one trace_id; `session.id`; span link to session |
| `slow-op` | `macos.report.render` INTERNAL 2.4s + `macos.hang.stack` event | WARN hang-suspected | duration histogram, 2400ms in (1000,5000], exemplar | one trace_id; `session.id` |
| `lifecycle` | `macos.app.session` + `macos.app.lifecycle.cold_start` child; `lifecycle.cold_start`/`foreground` events | INFO launching + active | `macos.playground.sessions` +1, exemplar | own trace_id; **same `session.id`** as failure/slow-op |
| `crash` | — (a crashed process cannot report itself) | — | — | `.ips` report + dSYM |

`--parent-traceparent 00-<trace>-<span>-01` joins a backend trace as a child
span in all three live scenarios. `--backend-url URL` makes a real
`URLSession` POST with the injected W3C header (verify.py uses a local 502
echo).

Resource attributes on every signal: `service.name/version/instance`,
`telemetry.sdk.*`, `os.type=darwin`, `os.name=macOS`, `os.description`,
`host.arch`, `device.model.identifier` (sysctl `hw.model`),
`process.executable.name`, `process.executable.build_id` + `macos.build_uuid`
(Mach-O `LC_UUID`, the dSYM key), `macos.app_version_source`,
`macos.thermal_state`, `macos.low_power_mode`, `macos.memory.physical_gb`,
`macos.memory.resident_bytes`, `macos.cpu.logical_count`, `macos.scenario_id`.

## Run

```bash
swift build
.build/debug/MacOSPlayground all --seed 42 --dry-run | head -c 600
.build/debug/MacOSPlayground failure --seed 42 --endpoint http://127.0.0.1:4318
```

Flags: `--seed`, `--frozen-time <unix_nanos>` (byte-determinism),
`--endpoint`, `--parent-traceparent`, `--service`, `--scenario-id`,
`--unified-log` (also emits `os_log`, verifiable via `log show`),
`--metrickit-probe-seconds N`, `--backend-url URL`.

## Verify

```bash
python3 tools/verify.py              # stub + crash + MetricKit blocker (no backend needed)
python3 tools/verify.py --parallax   # + live Parallax ingest/query (fails if none up)
```

Without `--parallax`, verify auto-joins a live Parallax if `/health` answers
on the usual pairs; otherwise it **skips** and documents that server join is
a separate step. Pairs: stock `4318/4000`, isolated `14328/14000`. Skips
token-walled instances. Only one *managed* Parallax fits per host (engine
ports 24000–24003 are hardcoded), so for the isolated leg start a second
engine + Parallax in `external` mode first:

```bash
greptime standalone start --http-addr 127.0.0.1:24100 --rpc-bind-addr 127.0.0.1:24101 \
  --mysql-addr 127.0.0.1:24102 --postgres-addr 127.0.0.1:24103 \
  --data-home /tmp/macotel-parallax/greptime-data &
parallax serve --config /tmp/macotel-parallax/config.toml &  # [storage] mode="external",
                                                            # greptime_url="http://127.0.0.1:24100",
                                                            # loopback bind, api_port=14000, otlp_http_port=14328
```

## Verified vs blocked (2026-09-13, macOS 26.6.2 arm64, Swift 6.3.3)

Verified (`tools/verify.py`, 38/38 green, 26 unit tests):

- One trace_id per operation joins span + logs + **metric exemplars**.
- Shared `session.id` across failure / slow-op / lifecycle; span link to the
  session span.
- Real `URLSession` POST injects W3C `traceparent` sharing the failure
  trace_id; local backend receives it.
- Unsigned `.app` with Info.plist yields `service.version=1.2.3 (45)` from
  the bundle (CLI fallback is `0.0.0-dev+cli`).
- Mach-O `LC_UUID` on the resource matches `dwarfdump --uuid`.
- Real `fatalError` crash → `.ips` → `atos` + dSYM DWARF resolves
  `static Harness.main() (Harness.swift:60)`.
- `os_log` roundtrip; W3C child-join; deterministic IDs.

Honestly blocked / out of scope (see also `docs/research/macos/`):

- **MetricKit payloads**: CLI probe **and** unsigned ad-hoc-signed `.app`
  both receive 0/0 payloads. Real `MXCrashDiagnostic` / `MXHangDiagnostic`
  delivery needs an installed **signed GUI app identity** plus Apple's
  aggregation delay. Not faked.
- **Server-side dSYM symbolication in Parallax**: no upload/symbolicate path
  exists; harness proves the client half (crash, dSYM, UUID, atos).
- **SwiftUI lifecycle/hang auto-instrumentation**: needs a real `.app` host;
  the harness emits equivalent lifecycle/hang telemetry manually.
