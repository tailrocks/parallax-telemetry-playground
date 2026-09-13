import Foundation

struct ScenarioConfig {
    var serviceName: String
    var scenarioId: String
    var seed: UInt64
    var frozenTimeNanos: UInt64?
    var parentTraceparent: String?
    var endpoint: String
    var scopeName: String = "com.tailrocks.macos-playground"
    var scopeVersion: String = "0.1.0"
}

struct ScenarioOutput {
    var name: String
    var traceIdHex: String
    var spanIdHex: String
    var traceparent: String
    var traces: Data
    var logs: Data
    var metrics: Data
}

func nowNanos(_ cfg: ScenarioConfig) -> UInt64 {
    if let f = cfg.frozenTimeNanos { return f }
    return UInt64(Date().timeIntervalSince1970 * 1_000_000_000)
}

/// Parses W3C `00-<tracehex>-<spanhex>-<flags>`; returns nil on malformed input.
func parseTraceparent(_ s: String) -> (traceId: Data, spanId: Data)? {
    let parts = s.split(separator: "-")
    guard parts.count == 4, parts[0] == "00",
        let t = dataFromHex(String(parts[1])), t.count == 16,
        let p = dataFromHex(String(parts[2])), p.count == 8
    else { return nil }
    return (t, p)
}

/// Failure scenario: a checkout submit from the native client fails.
/// One trace_id joins the client span (error + exception event with a REAL
/// captured stack), the error log record, and the failure counter.
func runFailureScenario(cfg: ScenarioConfig, native: NativeContext) -> ScenarioOutput {
    var rng = SplitMix64(seed: cfg.seed ^ 0xF411_0001)
    return runFailureScenario(cfg: cfg, native: native, rng: &rng)
}

private func runFailureScenario(cfg: ScenarioConfig, native: NativeContext, rng: inout SplitMix64) -> ScenarioOutput {
    let now = nowNanos(cfg)
    let start = now - 185_000_000 // 185ms operation
    var traceId = rng.bytes(count: 16)
    var parentId: Data?
    if let pt = cfg.parentTraceparent, let parsed = parseTraceparent(pt) {
        traceId = parsed.traceId // join backend trace as a child
        parentId = parsed.spanId
    }
    let spanId = rng.bytes(count: 8)
    let stack = captureStackTrace()

    let span = OtlpSpan(
        traceId: traceId, spanId: spanId, parentSpanId: parentId,
        name: "macos.checkout.submit", kind: 3 /* CLIENT */,
        startNanos: start, endNanos: now,
        stringAttrs: [
            ("http.request.method", "POST"),
            ("url.full", "http://localhost:8088/checkout"),
            ("error.type", "CheckoutDeclined"),
            ("macos.scenario", "failure"),
            ("macos.thermal_state", native.thermalState),
        ],
        intAttrs: [("http.response.status_code", 502)],
        events: [OtlpEvent(
            timeNanos: now,
            name: "exception",
            stringAttrs: [
                ("exception.type", "CheckoutDeclined"),
                ("exception.message", "payment backend returned 502 for seeded order"),
                ("exception.stacktrace", stack),
            ]
        )],
        statusCode: 2 /* ERROR */,
        statusMessage: "CheckoutDeclined: payment backend returned 502"
    )

    let traceHex = hexString(traceId)
    let spanHex = hexString(spanId)
    let logs = [
        OtlpLog(
            timeNanos: now, observedNanos: now,
            severityNumber: 17, severityText: "ERROR",
            body: "checkout submit failed: CheckoutDeclined (payment backend 502)",
            stringAttrs: [
                ("macos.scenario", "failure"),
                ("macos.scenario_id", cfg.scenarioId),
                ("error.type", "CheckoutDeclined"),
                ("code.function", "runFailureScenario"),
            ],
            traceId: traceId, spanId: spanId
        ),
        OtlpLog(
            timeNanos: start, observedNanos: start,
            severityNumber: 9, severityText: "INFO",
            body: "macos lifecycle: app active, checkout flow started",
            stringAttrs: [
                ("macos.scenario", "failure"),
                ("macos.lifecycle.state", "active"),
                ("macos.thermal_state", native.thermalState),
            ],
            traceId: traceId, spanId: spanId
        ),
    ]

    let res = native.resourceAttrs(serviceName: cfg.serviceName, scenarioId: cfg.scenarioId, stableInstance: cfg.frozenTimeNanos != nil)
    let traces = buildTracesData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, spans: [span])
    let logsData = buildLogsData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, logs: logs)
    let metrics = buildMetricsData(
        resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion,
        sums: [(
            name: "macos.playground.failures",
            description: "Failed native operations by scenario",
            unit: "{failure}",
            monotonic: true,
            points: [OtlpSumPoint(
                timeNanos: now, startNanos: start, value: 1,
                stringAttrs: [("macos.scenario", "failure"), ("error.type", "CheckoutDeclined")]
            )]
        )],
        histograms: []
    )
    return ScenarioOutput(
        name: "failure", traceIdHex: traceHex, spanIdHex: spanHex,
        traceparent: "00-\(traceHex)-\(spanHex)-01",
        traces: traces, logs: logsData, metrics: metrics
    )
}

/// Slow-op scenario: a main-thread-bound report render stalls (hang-like).
/// Long-duration span + WARN log + duration histogram share one trace_id.
func runSlowOpScenario(cfg: ScenarioConfig, native: NativeContext) -> ScenarioOutput {
    var rng = SplitMix64(seed: cfg.seed ^ 0x5_1002)
    let now = nowNanos(cfg)
    let durationNanos: UInt64 = 2_400_000_000 // 2.4s stall
    let start = now - durationNanos
    var traceId = rng.bytes(count: 16)
    var parentId: Data?
    if let pt = cfg.parentTraceparent, let parsed = parseTraceparent(pt) {
        traceId = parsed.traceId // join backend trace as a child
        parentId = parsed.spanId
    }
    let spanId = rng.bytes(count: 8)

    let span = OtlpSpan(
        traceId: traceId, spanId: spanId, parentSpanId: parentId,
        name: "macos.report.render", kind: 1 /* INTERNAL */,
        startNanos: start, endNanos: now,
        stringAttrs: [
            ("macos.scenario", "slow-op"),
            ("macos.hang.suspected", "true"),
            ("macos.thread", "main"),
        ],
        intAttrs: [("macos.operation.duration_ms", 2400)],
        events: [],
        statusCode: 0, statusMessage: ""
    )

    let traceHex = hexString(traceId)
    let spanHex = hexString(spanId)
    let logs = [OtlpLog(
        timeNanos: now, observedNanos: now,
        severityNumber: 13, severityText: "WARN",
        body: "main-thread render took 2400ms (stall threshold 500ms)",
        stringAttrs: [
            ("macos.scenario", "slow-op"),
            ("macos.hang.suspected", "true"),
        ],
        traceId: traceId, spanId: spanId
    )]

    let res = native.resourceAttrs(serviceName: cfg.serviceName, scenarioId: cfg.scenarioId, stableInstance: cfg.frozenTimeNanos != nil)
    let traces = buildTracesData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, spans: [span])
    let logsData = buildLogsData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, logs: logs)
    // 2400ms falls in the (1000, 5000] bucket (index 4 of 6 buckets).
    let metrics = buildMetricsData(
        resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion,
        sums: [],
        histograms: [(
            name: "macos.playground.operation.duration",
            description: "Native operation duration by scenario",
            unit: "ms",
            points: [OtlpHistogramPoint(
                timeNanos: now, startNanos: start, count: 1, sum: 2400,
                bounds: [10, 50, 100, 500, 1000, 5000],
                bucketCounts: [0, 0, 0, 0, 0, 1, 0],
                stringAttrs: [("macos.scenario", "slow-op")]
            )]
        )]
    )
    return ScenarioOutput(
        name: "slow-op", traceIdHex: traceHex, spanIdHex: spanHex,
        traceparent: "00-\(traceHex)-\(spanHex)-01",
        traces: traces, logs: logsData, metrics: metrics
    )
}
