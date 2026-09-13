import XCTest
@testable import MacOSPlayground

final class OtlpProtoTests: XCTestCase {
    func testSplitMixDeterministic() {
        var a = SplitMix64(seed: 42), b = SplitMix64(seed: 42)
        XCTAssertEqual(a.bytes(count: 16), b.bytes(count: 16))
        var c = SplitMix64(seed: 43)
        XCTAssertNotEqual(a.bytes(count: 16), c.bytes(count: 16))
    }

    func testIdsNeverAllZero() {
        // Sweep seeds; OTLP rejects all-zero trace/span IDs.
        for seed in [0, 1, 255, UInt64.max] as [UInt64] {
            var rng = SplitMix64(seed: seed)
            for _ in 0 ..< 100 {
                XCTAssertFalse(rng.bytes(count: 16).allSatisfy { $0 == 0 })
                XCTAssertFalse(rng.bytes(count: 8).allSatisfy { $0 == 0 })
            }
        }
    }

    func testBytesEdgeCounts() {
        var rng = SplitMix64(seed: 5)
        XCTAssertEqual(rng.bytes(count: 0), Data())
        XCTAssertEqual(rng.bytes(count: 4).count, 4) // non-multiple of 8 trims
        XCTAssertEqual(rng.bytes(count: 8).count, 8)
        XCTAssertFalse(rng.bytes(count: 4).allSatisfy { $0 == 0 })
    }

    func testHexRoundTrip() {
        var rng = SplitMix64(seed: 7)
        let d = rng.bytes(count: 16)
        XCTAssertEqual(dataFromHex(hexString(d)), d)
        XCTAssertNil(dataFromHex("zz"))
        XCTAssertNil(dataFromHex("abc"))
    }

    func testParseTraceparent() {
        let tp = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        let parsed = parseTraceparent(tp)
        XCTAssertNotNil(parsed)
        XCTAssertEqual(hexString(parsed!.traceId), "4bf92f3577b34da6a3ce929d0e0e4736")
        XCTAssertEqual(hexString(parsed!.spanId), "00f067aa0ba902b7")
        XCTAssertNil(parseTraceparent("bogus"))
        XCTAssertNil(parseTraceparent("00-short-00f067aa0ba902b7-01"))
    }

    func testTracesPayloadStructure() {
        // tag(1, wire2)=0x0A opens resource_spans; trace_id field tag is 0x0A len 0x10.
        let traceId = Data(repeating: 0xAB, count: 16)
        let spanId = Data(repeating: 0xCD, count: 8)
        let data = buildTracesData(
            resourceAttrs: [("service.name", "svc")],
            scopeName: "scope", scopeVersion: "1",
            spans: [OtlpSpan(
                traceId: traceId, spanId: spanId, parentSpanId: nil,
                name: "op", kind: 1, startNanos: 100, endNanos: 200,
                stringAttrs: [("k", "v")], intAttrs: [("n", 3)],
                events: [OtlpEvent(timeNanos: 150, name: "exception", stringAttrs: [("exception.type", "E")])],
                statusCode: 2, statusMessage: "boom"
            )]
        )
        XCTAssertEqual(data[data.startIndex], 0x0A)
        XCTAssertTrue(data.range(of: traceId) != nil)
        XCTAssertTrue(data.range(of: spanId) != nil)
        XCTAssertTrue(data.range(of: Data("op".utf8)) != nil)
        XCTAssertTrue(data.range(of: Data("svc".utf8)) != nil)
        XCTAssertTrue(data.range(of: Data("boom".utf8)) != nil)
    }

    func testScenariosDeterministic() {
        let native = NativeContext.collect()
        let cfg = ScenarioConfig(
            serviceName: "svc", scenarioId: "s", seed: 123,
            frozenTimeNanos: 1_700_000_000_000_000_000, parentTraceparent: nil,
            endpoint: "http://127.0.0.1:4318"
        )
        let a = runFailureScenario(cfg: cfg, native: native)
        // Stack capture differs run to run only in addresses, not membership of
        // the scenario function; IDs and timing must be identical.
        XCTAssertEqual(a.traceIdHex, runFailureScenario(cfg: cfg, native: native).traceIdHex)
        XCTAssertEqual(a.spanIdHex.count, 16)
        XCTAssertEqual(a.traceIdHex.count, 32)
        XCTAssertTrue(a.traceparent.hasPrefix("00-\(a.traceIdHex)-\(a.spanIdHex)-"))
        let s = runSlowOpScenario(cfg: cfg, native: native)
        XCTAssertEqual(s.traceIdHex, runSlowOpScenario(cfg: cfg, native: native).traceIdHex)
        XCTAssertNotEqual(a.traceIdHex, s.traceIdHex)
    }

    func testOtlpFieldNumbers() {
        // Regression: events=11 (not links=13), Status.code=3/message=2,
        // LogRecord.trace_id=9/span_id=10. Verified against
        // opentelemetry-proto 0.32.0 vendored .proto files.
        let traceId = Data(repeating: 0xAB, count: 16)
        let spanId = Data(repeating: 0xCD, count: 8)
        let traces = buildTracesData(
            resourceAttrs: [], scopeName: "s", scopeVersion: "1",
            spans: [OtlpSpan(
                traceId: traceId, spanId: spanId, parentSpanId: nil,
                name: "op", kind: 1, startNanos: 1, endNanos: 2,
                stringAttrs: [], intAttrs: [],
                events: [OtlpEvent(timeNanos: 1, name: "exception", stringAttrs: [])],
                statusCode: 2, statusMessage: "m"
            )]
        )
        // event envelope tag: field 11 wire 2 => 0x5A.
        XCTAssertTrue(traces.range(of: Data([0x5A])) != nil)
        // no links envelope (field 13 wire 2 => 0x6A) may appear.
        XCTAssertTrue(traces.range(of: Data([0x6A])) == nil)
        // status code tag: field 3 varint => 0x18 0x02.
        XCTAssertTrue(traces.range(of: Data([0x18, 0x02])) != nil)
        let logs = buildLogsData(
            resourceAttrs: [], scopeName: "s", scopeVersion: "1",
            logs: [OtlpLog(
                timeNanos: 1, observedNanos: 1, severityNumber: 9,
                severityText: "INFO", body: "b", stringAttrs: [],
                traceId: traceId, spanId: spanId
            )]
        )
        // trace_id tag: field 9 wire 2 => 0x4A len 0x10; span_id: 0x52 len 8.
        XCTAssertTrue(logs.range(of: Data([0x4A, 0x10])) != nil)
        XCTAssertTrue(logs.range(of: Data([0x52, 0x08])) != nil)
    }

    func testParentTraceparentJoinsBackendTrace() {
        let native = NativeContext.collect()
        let cfg = ScenarioConfig(
            serviceName: "svc", scenarioId: "s", seed: 9,
            frozenTimeNanos: 1_700_000_000_000_000_000,
            parentTraceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            endpoint: "http://127.0.0.1:4318"
        )
        let a = runFailureScenario(cfg: cfg, native: native)
        XCTAssertEqual(a.traceIdHex, "4bf92f3577b34da6a3ce929d0e0e4736")
        // parent_span_id must be encoded: field 4 wire 2 => tag 0x22 len 8 + id bytes.
        var needle = Data([0x22, 0x08])
        needle.append(dataFromHex("00f067aa0ba902b7")!)
        XCTAssertTrue(a.traces.range(of: needle) != nil)
    }

    func testSlowOpParentTraceparentJoinsBackendTrace() {
        let native = NativeContext.collect()
        let cfg = ScenarioConfig(
            serviceName: "svc", scenarioId: "s", seed: 9,
            frozenTimeNanos: 1_700_000_000_000_000_000,
            parentTraceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            endpoint: "http://127.0.0.1:4318"
        )
        let s = runSlowOpScenario(cfg: cfg, native: native)
        XCTAssertEqual(s.traceIdHex, "4bf92f3577b34da6a3ce929d0e0e4736")
        var needle = Data([0x22, 0x08])
        needle.append(dataFromHex("00f067aa0ba902b7")!)
        XCTAssertTrue(s.traces.range(of: needle) != nil)
    }

    func testResourceAttrsSemconv() {
        let native = NativeContext.collect()
        let live = native.resourceAttrs(serviceName: "svc", scenarioId: "sid")
        let dict = Dictionary(uniqueKeysWithValues: live)
        XCTAssertEqual(dict["service.name"], "svc")
        XCTAssertEqual(dict["os.type"], "darwin")
        XCTAssertEqual(dict["macos.scenario_id"], "sid")
        XCTAssertEqual(dict["service.instance.id"], "\(native.processName)-\(native.pid)")
        let frozen = native.resourceAttrs(serviceName: "svc", scenarioId: "sid", stableInstance: true)
        XCTAssertTrue(frozen.contains { $0.0 == "service.instance.id" && $0.1.hasSuffix("-frozen") })
    }

    func testMetricsSumStructure() {
        let data = buildMetricsData(
            resourceAttrs: [], scopeName: "s", scopeVersion: "1",
            sums: [(name: "m", description: "d", unit: "{f}", monotonic: true, points: [OtlpSumPoint(timeNanos: 1, startNanos: 0, value: 1, stringAttrs: [("k", "v")])])],
            histograms: []
        )
        // Sum envelope: Metric.sum field 7 wire 2 => 0x3A; as_double field 4 wire 1 => 0x21;
        // aggregation_temporality field 2 varint => 0x10 0x02; is_monotonic 0x18 0x01.
        XCTAssertTrue(data.range(of: Data([0x3A])) != nil)
        XCTAssertTrue(data.range(of: Data([0x21])) != nil)
        XCTAssertTrue(data.range(of: Data([0x10, 0x02])) != nil)
        XCTAssertTrue(data.range(of: Data([0x18, 0x01])) != nil)
        XCTAssertTrue(data.range(of: Data("m".utf8)) != nil)
        XCTAssertTrue(data.range(of: Data("v".utf8)) != nil)
    }

    func testCaptureStackTraceTruncation() {
        let truncated = captureStackTrace(maxChars: 50)
        XCTAssertTrue(truncated.contains("... (truncated)"))
        XCTAssertEqual(truncated.prefix(50).count, 50)
        let full = captureStackTrace(maxChars: 1_000_000)
        XCTAssertFalse(full.contains("... (truncated)"))
        XCTAssertFalse(full.isEmpty)
    }

    func testNativeContextReal() {
        let n = NativeContext.collect()
        XCTAssertTrue(n.osVersion.contains("Version"))
        XCTAssertFalse(n.deviceModel.isEmpty)
        XCTAssertGreaterThan(n.processorCount, 0)
        XCTAssertFalse(captureStackTrace().isEmpty)
    }
}
