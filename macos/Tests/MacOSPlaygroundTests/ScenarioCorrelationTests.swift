import XCTest
@testable import MacOSPlayground

final class ScenarioCorrelationTests: XCTestCase {
    func cfg(_ seed: UInt64 = 123, parent: String? = nil) -> ScenarioConfig {
        ScenarioConfig(
            serviceName: "svc", scenarioId: "s", seed: seed,
            frozenTimeNanos: 1_700_000_000_000_000_000, parentTraceparent: parent,
            endpoint: "http://127.0.0.1:4318"
        )
    }

    func testFailureHangLifecycleShareOneTraceId() {
        let native = NativeContext.collect()
        let out = runSessionTraceScenario(cfg: cfg(), native: native)
        XCTAssertEqual(out.name, "session")
        XCTAssertEqual(out.traceIdHex.count, 32)
        let tid = dataFromHex(out.traceIdHex)!
        XCTAssertTrue(out.traces.range(of: Data("macos.app.session".utf8)) != nil)
        XCTAssertTrue(out.traces.range(of: Data("macos.checkout.submit".utf8)) != nil)
        XCTAssertTrue(out.traces.range(of: Data("macos.report.render".utf8)) != nil)
        XCTAssertTrue(out.traces.range(of: tid) != nil)
        XCTAssertTrue(out.logs.range(of: tid) != nil)
        XCTAssertTrue(out.metrics.range(of: tid) != nil)
        XCTAssertTrue(out.logs.range(of: Data("checkout submit failed".utf8)) != nil)
        XCTAssertTrue(out.logs.range(of: Data("main-thread render".utf8)) != nil)
        XCTAssertTrue(out.logs.range(of: Data("cold start".utf8)) != nil)
    }

    func testSessionIdSharedAcrossScenarios() {
        let native = NativeContext.collect()
        let c = cfg()
        let f = runFailureScenario(cfg: c, native: native)
        let s = runSlowOpScenario(cfg: c, native: native)
        let l = runLifecycleScenario(cfg: c, native: native)
        XCTAssertEqual(f.sessionId, s.sessionId)
        XCTAssertEqual(f.sessionId, l.sessionId)
        XCTAssertEqual(f.sessionId.count, 32)
        XCTAssertNotEqual(f.traceIdHex, s.traceIdHex)
        XCTAssertNotEqual(f.traceIdHex, l.traceIdHex)
        XCTAssertEqual(f.sessionId, sessionId(for: c))
    }

    func testLifecycleDeterministic() {
        let native = NativeContext.collect()
        let c = cfg()
        let a = runLifecycleScenario(cfg: c, native: native)
        let b = runLifecycleScenario(cfg: c, native: native)
        XCTAssertEqual(a.traceIdHex, b.traceIdHex)
        XCTAssertEqual(a.spanIdHex, b.spanIdHex)
        XCTAssertTrue(a.traces.range(of: Data("macos.app.session".utf8)) != nil)
        XCTAssertTrue(a.traces.range(of: Data("macos.app.lifecycle.cold_start".utf8)) != nil)
        XCTAssertTrue(a.traces.range(of: Data("lifecycle.cold_start".utf8)) != nil)
        XCTAssertTrue(a.traces.range(of: Data("lifecycle.foreground".utf8)) != nil)
        XCTAssertTrue(a.metrics.range(of: Data("macos.playground.sessions".utf8)) != nil)
    }

    func testLifecycleParentTraceparentJoinsBackendTrace() {
        let native = NativeContext.collect()
        let c = cfg(parent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        let l = runLifecycleScenario(cfg: c, native: native)
        XCTAssertEqual(l.traceIdHex, "4bf92f3577b34da6a3ce929d0e0e4736")
        var needle = Data([0x22, 0x08])
        needle.append(dataFromHex("00f067aa0ba902b7")!)
        XCTAssertTrue(l.traces.range(of: needle) != nil)
    }

    func testFailureInjectsTraceparentOnHttpChild() {
        let native = NativeContext.collect()
        let c = cfg()
        let f = runFailureScenario(cfg: c, native: native)
        let injected = failureClientTraceparent(cfg: c)
        XCTAssertTrue(injected.hasPrefix("00-\(f.traceIdHex)-"))
        XCTAssertTrue(injected.hasSuffix("-01"))
        XCTAssertTrue(f.traces.range(of: Data("HTTP POST".utf8)) != nil)
        XCTAssertTrue(f.traces.range(of: Data("http.request.header.traceparent".utf8)) != nil)
        XCTAssertTrue(f.traces.range(of: Data(injected.utf8)) != nil)
        XCTAssertTrue(f.traces.range(of: Data("macos.checkout.submit".utf8)) != nil)
    }

    func testFailureAndSlowOpExemplarsCarryTraceId() {
        let native = NativeContext.collect()
        let c = cfg()
        let f = runFailureScenario(cfg: c, native: native)
        let s = runSlowOpScenario(cfg: c, native: native)
        XCTAssertTrue(f.metrics.range(of: dataFromHex(f.traceIdHex)!) != nil)
        XCTAssertTrue(s.metrics.range(of: dataFromHex(s.traceIdHex)!) != nil)
        // HistogramDataPoint.exemplars field 8 wire 2 => 0x42.
        XCTAssertTrue(f.metrics.range(of: Data([0x42])) != nil)
        XCTAssertTrue(s.metrics.range(of: Data([0x42])) != nil)
    }

    func testFailureLinksToSessionSpan() {
        let native = NativeContext.collect()
        let c = cfg()
        let f = runFailureScenario(cfg: c, native: native)
        let l = runLifecycleScenario(cfg: c, native: native)
        XCTAssertTrue(f.traces.range(of: dataFromHex(l.traceIdHex)!) != nil)
        XCTAssertTrue(f.traces.range(of: dataFromHex(l.spanIdHex)!) != nil)
        XCTAssertTrue(f.traces.range(of: Data([0x6A])) != nil)
    }

    func testSlowOpHangStackEvent() {
        let native = NativeContext.collect()
        let s = runSlowOpScenario(cfg: cfg(), native: native)
        XCTAssertTrue(s.traces.range(of: Data("macos.hang.stack".utf8)) != nil)
        XCTAssertTrue(s.traces.range(of: Data("exception.stacktrace".utf8)) != nil)
        XCTAssertTrue(s.logs.range(of: Data("session.id".utf8)) != nil)
    }

    func testErrorLogCarriesStack() {
        let native = NativeContext.collect()
        let f = runFailureScenario(cfg: cfg(), native: native)
        XCTAssertTrue(f.logs.range(of: Data("exception.stacktrace".utf8)) != nil)
        XCTAssertTrue(f.logs.range(of: Data("CheckoutDeclined".utf8)) != nil)
        XCTAssertTrue(f.logs.range(of: Data(f.sessionId.utf8)) != nil)
    }

    func testResourceReleaseContextOnEverySignal() {
        let native = NativeContext.collect()
        let f = runFailureScenario(cfg: cfg(), native: native)
        XCTAssertTrue(f.traces.range(of: Data("macos.build_uuid".utf8)) != nil)
        XCTAssertTrue(f.logs.range(of: Data("macos.build_uuid".utf8)) != nil)
        XCTAssertTrue(f.metrics.range(of: Data("macos.build_uuid".utf8)) != nil)
        XCTAssertTrue(f.traces.range(of: Data("service.version".utf8)) != nil)
        XCTAssertTrue(f.metrics.range(of: Data("macos.playground.failures".utf8)) != nil)
        XCTAssertTrue(f.metrics.range(of: Data("macos.playground.operation.duration".utf8)) != nil)
    }
}
