import Foundation

// Deterministic macOS telemetry harness. Emits hand-encoded OTLP/HTTP
// protobuf (Parallax accepts protobuf only) for a native failure and a
// slow operation, joining traces/logs/metrics on one trace_id each.

@main
struct Harness {
    static func main() async {
        let args = Array(CommandLine.arguments.dropFirst())
        var mode = "all"
        var seed: UInt64 = UInt64.random(in: 0 ... UInt64.max)
        var seedGiven = false
        var endpoint = "http://127.0.0.1:4318"
        var frozen: UInt64?
        var dryRun = false
        var parent: String?
        var service = "macos-playground"
        var scenarioId: String?
        var unifiedLog = false
        var metricKitSeconds: UInt32 = 0

        var i = 0
        while i < args.count {
            let a = args[i]
            func nextValue(_ flag: String) -> String {
                guard i + 1 < args.count else { fatalError("missing value for \(flag)") }
                i += 1
                return args[i]
            }
            switch a {
            case "failure", "slow-op", "all", "crash": mode = a
            case "--seed": seed = UInt64(nextValue(a)) ?? 0; seedGiven = true
            case "--endpoint": endpoint = nextValue(a)
            case "--frozen-time": frozen = UInt64(nextValue(a))
            case "--dry-run": dryRun = true
            case "--parent-traceparent": parent = nextValue(a)
            case "--service": service = nextValue(a)
            case "--scenario-id": scenarioId = nextValue(a)
            case "--unified-log": unifiedLog = true
            case "--metrickit-probe-seconds": metricKitSeconds = UInt32(nextValue(a)) ?? 0
            case "-h", "--help":
                print(usage)
                return
            default: fatalError("unknown argument: \(a)\n\(usage)")
            }
            i += 1
        }

        if mode == "crash" {
            // Deliberate deterministic crash for the .ips + dSYM symbolication proof.
            // No telemetry is emitted here by design: a crashed process cannot
            // report itself; capture happens out-of-process (DiagnosticReports).
            let preamble = ["mode": "crash", "pid": "\(ProcessInfo.processInfo.processIdentifier)", "marker": "macos-playground-crash"]
            print(String(data: try! JSONSerialization.data(withJSONObject: preamble), encoding: .utf8)!)
            fflush(stdout)
            try? await Task.sleep(nanoseconds: 200_000_000)
            fatalError("macos-playground deterministic crash (marker=macos-playground-crash)")
        }

        if let p = parent, parseTraceparent(p) == nil {
            fatalError("malformed --parent-traceparent, want 00-<32hex>-<16hex>-<2hex>")
        }

        let native = NativeContext.collect()
        let sid = scenarioId ?? "macos-\(seed)"
        let cfg = ScenarioConfig(
            serviceName: service, scenarioId: sid, seed: seed,
            frozenTimeNanos: frozen, parentTraceparent: parent, endpoint: endpoint
        )

        var outputs: [ScenarioOutput] = []
        if mode == "failure" || mode == "all" { outputs.append(runFailureScenario(cfg: cfg, native: native)) }
        if mode == "slow-op" || mode == "all" { outputs.append(runSlowOpScenario(cfg: cfg, native: native)) }

        if unifiedLog {
            for o in outputs {
                emitUnifiedLog(scenario: o.name, traceIdHex: o.traceIdHex, marker: "macos-playground-\(sid)")
            }
        }

        var metricKit: [String: Any] = ["probed": false]
        if metricKitSeconds > 0 {
            let r = MetricKitProbe.run(waitSeconds: metricKitSeconds)
            metricKit = ["probed": true, "wait_seconds": metricKitSeconds, "metric_payloads": r.metrics, "diagnostic_payloads": r.diagnostics]
        }

        var posted: [String: Any] = [:]
        if !dryRun {
            for o in outputs {
                posted[o.name] = [
                    "traces": await post(endpoint: endpoint, signal: "traces", body: o.traces),
                    "logs": await post(endpoint: endpoint, signal: "logs", body: o.logs),
                    "metrics": await post(endpoint: endpoint, signal: "metrics", body: o.metrics),
                ]
            }
        }

        var summary: [String: Any] = [
            "mode": mode,
            "seed": seed,
            "seed_given": seedGiven,
            "scenario_id": sid,
            "endpoint": endpoint,
            "dry_run": dryRun,
            "service": service,
            "native": [
                "os": native.osVersion,
                "thermal_state": native.thermalState,
                "device_model": native.deviceModel,
                "app_version": native.appVersion,
                "app_version_source": native.appVersionSource,
                "pid": native.pid,
            ],
            "metrickit": metricKit,
            "scenarios": outputs.map { o in
                [
                    "name": o.name,
                    "trace_id": o.traceIdHex,
                    "span_id": o.spanIdHex,
                    "traceparent": o.traceparent,
                    "bytes": ["traces": o.traces.count, "logs": o.logs.count, "metrics": o.metrics.count],
                ] as [String: Any]
            },
            "posted": posted,
        ]
        if dryRun {
            summary["payloads"] = outputs.map { o in
                [
                    "name": o.name,
                    "traces_b64": o.traces.base64EncodedString(),
                    "logs_b64": o.logs.base64EncodedString(),
                    "metrics_b64": o.metrics.base64EncodedString(),
                ]
            }
        }
        let data = try! JSONSerialization.data(withJSONObject: summary, options: [.sortedKeys])
        print(String(data: data, encoding: .utf8)!)
    }

    static func post(endpoint: String, signal: String, body: Data) async -> [String: Any] {
        guard let url = URL(string: "\(endpoint)/v1/\(signal)") else {
            return ["ok": false, "error": "bad endpoint"]
        }
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.timeoutInterval = 15
        req.setValue("application/x-protobuf", forHTTPHeaderField: "Content-Type")
        req.httpBody = body
        do {
            let (_, resp) = try await URLSession.shared.data(for: req)
            let code = (resp as? HTTPURLResponse)?.statusCode ?? -1
            return ["ok": code >= 200 && code < 300, "status": code]
        } catch {
            return ["ok": false, "error": "\(error)"]
        }
    }
}

let usage = """
usage: MacOSPlayground [failure|slow-op|all|crash] [flags]
  --seed N                   deterministic ID seed (default: random)
  --endpoint URL             OTLP/HTTP base (default http://127.0.0.1:4318)
  --frozen-time NANOS        fixed clock for byte-determinism checks
  --dry-run                  print payloads, do not POST
  --parent-traceparent TP    join backend trace as child span
  --service NAME             service.name (default macos-playground)
  --scenario-id ID           macos.scenario_id (default macos-<seed>)
  --unified-log              also emit os_log entries (verifiable via `log show`)
  --metrickit-probe-seconds N  subscribe to MetricKit and report payloads
"""
