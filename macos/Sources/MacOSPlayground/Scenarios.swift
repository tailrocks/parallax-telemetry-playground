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
    var sessionId: String
    var traces: Data
    var logs: Data
    var metrics: Data
}

enum ScenarioSalt {
    static let failure: UInt64 = 0xF411_0001
    static let slowOp: UInt64 = 0x5_1002
    static let lifecycle: UInt64 = 0x11FE_0001
    static let session: UInt64 = 0x5E55_1001
}

let durationBounds: [Double] = [10, 50, 100, 500, 1000, 5000]

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

func formatTraceparent(traceId: Data, spanId: Data) -> String {
    "00-\(hexString(traceId))-\(hexString(spanId))-01"
}

/// Deterministic session.id shared by every scenario for one harness seed.
func sessionId(for cfg: ScenarioConfig) -> String {
    var rng = SplitMix64(seed: cfg.seed ^ ScenarioSalt.session)
    return hexString(rng.bytes(count: 16))
}

func mintTraceIds(cfg: ScenarioConfig, salt: UInt64) -> (traceId: Data, spanId: Data, parentId: Data?, rng: SplitMix64) {
    var rng = SplitMix64(seed: cfg.seed ^ salt)
    var traceId = rng.bytes(count: 16)
    var parentId: Data?
    if let pt = cfg.parentTraceparent, let parsed = parseTraceparent(pt) {
        traceId = parsed.traceId
        parentId = parsed.spanId
    }
    let spanId = rng.bytes(count: 8)
    return (traceId, spanId, parentId, rng)
}

func durationBuckets(_ ms: Double) -> [UInt64] {
    var counts = [UInt64](repeating: 0, count: durationBounds.count + 1)
    let idx = durationBounds.firstIndex(where: { ms <= $0 }) ?? durationBounds.count
    counts[idx] = 1
    return counts
}

func makeExemplar(now: UInt64, value: Double, traceId: Data, spanId: Data, scenario: String) -> OtlpExemplar {
    OtlpExemplar(
        timeNanos: now, value: value, traceId: traceId, spanId: spanId,
        stringAttrs: [("macos.scenario", scenario)]
    )
}

func sessionLink(cfg: ScenarioConfig) -> OtlpLink {
    let minted = mintTraceIds(cfg: cfg, salt: ScenarioSalt.lifecycle)
    return OtlpLink(
        traceId: minted.traceId, spanId: minted.spanId,
        stringAttrs: [("session.id", sessionId(for: cfg))]
    )
}

/// W3C header the failure CLIENT span injects on the outbound checkout POST.
func failureClientTraceparent(cfg: ScenarioConfig) -> String {
    let minted = mintTraceIds(cfg: cfg, salt: ScenarioSalt.failure)
    var rng = minted.rng
    let childSpanId = rng.bytes(count: 8)
    return formatTraceparent(traceId: minted.traceId, spanId: childSpanId)
}

/// Failure scenario: native checkout submit fails after an outbound HTTP POST.
/// One trace_id joins the INTERNAL submit span (error + exception + real stack),
/// the CLIENT HTTP child (injected `traceparent`), ERROR+INFO logs, failure
/// counter, duration histogram, and histogram/counter exemplars.
func runFailureScenario(cfg: ScenarioConfig, native: NativeContext, httpStatus: Int = 502, backendUrl: String = "http://localhost:8088/checkout") -> ScenarioOutput {
    let now = nowNanos(cfg)
    let durationMs: Double = 185
    let start = now - 185_000_000
    let minted = mintTraceIds(cfg: cfg, salt: ScenarioSalt.failure)
    var rng = minted.rng
    let traceId = minted.traceId
    let spanId = minted.spanId
    let parentId = minted.parentId
    let httpSpanId = rng.bytes(count: 8)
    let stack = captureStackTrace()
    let sid = sessionId(for: cfg)
    let injected = formatTraceparent(traceId: traceId, spanId: httpSpanId)
    let isError = httpStatus >= 400 || httpStatus < 0

    let submit = OtlpSpan(
        traceId: traceId, spanId: spanId, parentSpanId: parentId,
        name: "macos.checkout.submit", kind: 1 /* INTERNAL */,
        startNanos: start, endNanos: now,
        stringAttrs: [
            ("macos.scenario", "failure"),
            ("macos.thermal_state", native.thermalState),
            ("session.id", sid),
            ("error.type", "CheckoutDeclined"),
            ("macos.build_uuid", native.buildUUID),
        ],
        intAttrs: [("macos.operation.duration_ms", Int64(durationMs))],
        events: [OtlpEvent(
            timeNanos: now,
            name: "exception",
            stringAttrs: [
                ("exception.type", "CheckoutDeclined"),
                ("exception.message", "payment backend returned \(httpStatus) for seeded order"),
                ("exception.stacktrace", stack),
            ]
        )],
        statusCode: isError ? 2 : 1,
        statusMessage: isError ? "CheckoutDeclined: payment backend returned \(httpStatus)" : "",
        links: [sessionLink(cfg: cfg)]
    )

    let http = OtlpSpan(
        traceId: traceId, spanId: httpSpanId, parentSpanId: spanId,
        name: "HTTP POST", kind: 3 /* CLIENT */,
        startNanos: now - 120_000_000, endNanos: now - 5_000_000,
        stringAttrs: [
            ("http.request.method", "POST"),
            ("url.full", backendUrl),
            ("http.request.header.traceparent", injected),
            ("macos.scenario", "failure"),
            ("session.id", sid),
            ("error.type", isError ? "CheckoutDeclined" : ""),
        ].filter { !$0.1.isEmpty },
        intAttrs: [("http.response.status_code", Int64(httpStatus))],
        events: [],
        statusCode: isError ? 2 : 1,
        statusMessage: isError ? "HTTP \(httpStatus)" : ""
    )

    let traceHex = hexString(traceId)
    let spanHex = hexString(spanId)
    let logs = [
        OtlpLog(
            timeNanos: now, observedNanos: now,
            severityNumber: 17, severityText: "ERROR",
            body: "checkout submit failed: CheckoutDeclined (payment backend \(httpStatus))",
            stringAttrs: [
                ("macos.scenario", "failure"),
                ("macos.scenario_id", cfg.scenarioId),
                ("session.id", sid),
                ("error.type", "CheckoutDeclined"),
                ("code.function", "runFailureScenario"),
                ("exception.stacktrace", stack),
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
                ("session.id", sid),
            ],
            traceId: traceId, spanId: spanId
        ),
    ]

    let res = native.resourceAttrs(serviceName: cfg.serviceName, scenarioId: cfg.scenarioId, stableInstance: cfg.frozenTimeNanos != nil)
    let traces = buildTracesData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, spans: [submit, http])
    let logsData = buildLogsData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, logs: logs)
    let ex = makeExemplar(now: now, value: durationMs, traceId: traceId, spanId: spanId, scenario: "failure")
    let metrics = buildMetricsData(
        resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion,
        sums: [(
            name: "macos.playground.failures",
            description: "Failed native operations by scenario",
            unit: "{failure}",
            monotonic: true,
            points: [OtlpSumPoint(
                timeNanos: now, startNanos: start, value: 1,
                stringAttrs: [("macos.scenario", "failure"), ("error.type", "CheckoutDeclined"), ("session.id", sid)],
                exemplars: [makeExemplar(now: now, value: 1, traceId: traceId, spanId: spanId, scenario: "failure")]
            )]
        )],
        histograms: [(
            name: "macos.playground.operation.duration",
            description: "Native operation duration by scenario",
            unit: "ms",
            points: [OtlpHistogramPoint(
                timeNanos: now, startNanos: start, count: 1, sum: durationMs,
                bounds: durationBounds,
                bucketCounts: durationBuckets(durationMs),
                stringAttrs: [("macos.scenario", "failure"), ("session.id", sid)],
                exemplars: [ex]
            )]
        )]
    )
    return ScenarioOutput(
        name: "failure", traceIdHex: traceHex, spanIdHex: spanHex,
        traceparent: formatTraceparent(traceId: traceId, spanId: spanId),
        sessionId: sid,
        traces: traces, logs: logsData, metrics: metrics
    )
}

/// Slow-op scenario: a main-thread-bound report render stalls (hang-like).
/// Long-duration span + WARN log + duration histogram + exemplar share one
/// trace_id. Hang stack is a real `Thread.callStackSymbols` sample.
func runSlowOpScenario(cfg: ScenarioConfig, native: NativeContext) -> ScenarioOutput {
    let now = nowNanos(cfg)
    let durationMs: Double = 2400
    let durationNanos: UInt64 = 2_400_000_000
    let start = now - durationNanos
    let minted = mintTraceIds(cfg: cfg, salt: ScenarioSalt.slowOp)
    let traceId = minted.traceId
    let spanId = minted.spanId
    let parentId = minted.parentId
    let sid = sessionId(for: cfg)
    let hangStack = captureStackTrace()

    let span = OtlpSpan(
        traceId: traceId, spanId: spanId, parentSpanId: parentId,
        name: "macos.report.render", kind: 1 /* INTERNAL */,
        startNanos: start, endNanos: now,
        stringAttrs: [
            ("macos.scenario", "slow-op"),
            ("macos.hang.suspected", "true"),
            ("macos.thread", "main"),
            ("macos.thermal_state", native.thermalState),
            ("macos.low_power_mode", native.lowPowerMode ? "true" : "false"),
            ("session.id", sid),
            ("macos.build_uuid", native.buildUUID),
        ],
        intAttrs: [("macos.operation.duration_ms", 2400), ("macos.hang.threshold_ms", 500)],
        events: [OtlpEvent(
            timeNanos: now,
            name: "macos.hang.stack",
            stringAttrs: [
                ("macos.hang.suspected", "true"),
                ("exception.stacktrace", hangStack),
            ]
        )],
        statusCode: 0, statusMessage: "",
        links: [sessionLink(cfg: cfg)]
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
            ("session.id", sid),
        ],
        traceId: traceId, spanId: spanId
    )]

    let res = native.resourceAttrs(serviceName: cfg.serviceName, scenarioId: cfg.scenarioId, stableInstance: cfg.frozenTimeNanos != nil)
    let traces = buildTracesData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, spans: [span])
    let logsData = buildLogsData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, logs: logs)
    let metrics = buildMetricsData(
        resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion,
        sums: [],
        histograms: [(
            name: "macos.playground.operation.duration",
            description: "Native operation duration by scenario",
            unit: "ms",
            points: [OtlpHistogramPoint(
                timeNanos: now, startNanos: start, count: 1, sum: durationMs,
                bounds: durationBounds,
                bucketCounts: durationBuckets(durationMs),
                stringAttrs: [("macos.scenario", "slow-op"), ("session.id", sid)],
                exemplars: [makeExemplar(now: now, value: durationMs, traceId: traceId, spanId: spanId, scenario: "slow-op")]
            )]
        )]
    )
    return ScenarioOutput(
        name: "slow-op", traceIdHex: traceHex, spanIdHex: spanHex,
        traceparent: formatTraceparent(traceId: traceId, spanId: spanId),
        sessionId: sid,
        traces: traces, logs: logsData, metrics: metrics
    )
}

/// Lifecycle scenario: session-as-span (Embrace model) with cold-start child
/// and foreground event. Same `session.id` as failure/slow-op so a developer
/// can pivot from an error to the enclosing native session.
func runLifecycleScenario(cfg: ScenarioConfig, native: NativeContext) -> ScenarioOutput {
    let now = nowNanos(cfg)
    let sessionNanos: UInt64 = 4_200_000_000
    let start = now - sessionNanos
    let coldEnd = start + 180_000_000
    let minted = mintTraceIds(cfg: cfg, salt: ScenarioSalt.lifecycle)
    var rng = minted.rng
    let traceId = minted.traceId
    let spanId = minted.spanId
    let parentId = minted.parentId
    let coldSpanId = rng.bytes(count: 8)
    let sid = sessionId(for: cfg)

    let session = OtlpSpan(
        traceId: traceId, spanId: spanId, parentSpanId: parentId,
        name: "macos.app.session", kind: 1 /* INTERNAL */,
        startNanos: start, endNanos: now,
        stringAttrs: [
            ("macos.scenario", "lifecycle"),
            ("session.id", sid),
            ("macos.lifecycle.state", "active"),
            ("macos.build_uuid", native.buildUUID),
            ("service.version", native.appVersion),
        ],
        intAttrs: [],
        events: [
            OtlpEvent(timeNanos: start, name: "lifecycle.cold_start", stringAttrs: [
                ("macos.lifecycle.state", "launching"),
                ("session.id", sid),
            ]),
            OtlpEvent(timeNanos: coldEnd, name: "lifecycle.foreground", stringAttrs: [
                ("macos.lifecycle.state", "active"),
                ("session.id", sid),
            ]),
        ],
        statusCode: 1, statusMessage: ""
    )

    let cold = OtlpSpan(
        traceId: traceId, spanId: coldSpanId, parentSpanId: spanId,
        name: "macos.app.lifecycle.cold_start", kind: 1 /* INTERNAL */,
        startNanos: start, endNanos: coldEnd,
        stringAttrs: [
            ("macos.scenario", "lifecycle"),
            ("macos.lifecycle.state", "launching"),
            ("session.id", sid),
        ],
        intAttrs: [("macos.operation.duration_ms", 180)],
        events: [],
        statusCode: 1, statusMessage: ""
    )

    let traceHex = hexString(traceId)
    let spanHex = hexString(spanId)
    let logs = [
        OtlpLog(
            timeNanos: start, observedNanos: start,
            severityNumber: 9, severityText: "INFO",
            body: "macos lifecycle: cold start",
            stringAttrs: [
                ("macos.scenario", "lifecycle"),
                ("macos.lifecycle.state", "launching"),
                ("session.id", sid),
            ],
            traceId: traceId, spanId: coldSpanId
        ),
        OtlpLog(
            timeNanos: coldEnd, observedNanos: coldEnd,
            severityNumber: 9, severityText: "INFO",
            body: "macos lifecycle: foreground / app active",
            stringAttrs: [
                ("macos.scenario", "lifecycle"),
                ("macos.lifecycle.state", "active"),
                ("session.id", sid),
            ],
            traceId: traceId, spanId: spanId
        ),
    ]

    let res = native.resourceAttrs(serviceName: cfg.serviceName, scenarioId: cfg.scenarioId, stableInstance: cfg.frozenTimeNanos != nil)
    let traces = buildTracesData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, spans: [session, cold])
    let logsData = buildLogsData(resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion, logs: logs)
    let metrics = buildMetricsData(
        resourceAttrs: res, scopeName: cfg.scopeName, scopeVersion: cfg.scopeVersion,
        sums: [(
            name: "macos.playground.sessions",
            description: "Native app sessions started",
            unit: "{session}",
            monotonic: true,
            points: [OtlpSumPoint(
                timeNanos: now, startNanos: start, value: 1,
                stringAttrs: [("macos.scenario", "lifecycle"), ("session.id", sid)],
                exemplars: [makeExemplar(now: now, value: 1, traceId: traceId, spanId: spanId, scenario: "lifecycle")]
            )]
        )],
        histograms: []
    )
    return ScenarioOutput(
        name: "lifecycle", traceIdHex: traceHex, spanIdHex: spanHex,
        traceparent: formatTraceparent(traceId: traceId, spanId: spanId),
        sessionId: sid,
        traces: traces, logs: logsData, metrics: metrics
    )
}
