import Darwin
import Foundation
import MetricKit
import os

/// Real native context collected from macOS APIs. No stubs: every value comes
/// from ProcessInfo/sysctl/Bundle, and unavailable values are labeled as such.
struct NativeContext {
    var osVersion: String
    var thermalState: String
    var processorCount: Int
    var physicalMemoryGB: Double
    var uptimeSeconds: Int
    var deviceModel: String
    var appVersion: String
    var appVersionSource: String
    var processName: String
    var pid: Int32

    static func collect() -> NativeContext {
        let p = ProcessInfo.processInfo
        let thermal: String
        switch p.thermalState {
        case .nominal: thermal = "nominal"
        case .fair: thermal = "fair"
        case .serious: thermal = "serious"
        case .critical: thermal = "critical"
        @unknown default: thermal = "unknown"
        }
        let bundle = Bundle.main
        let shortVersion = bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        let build = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        let (version, source): (String, String)
        if let sv = shortVersion, let b = build {
            version = "\(sv) (\(b))"; source = "bundle"
        } else {
            // A raw CLI binary has no Info.plist; a shipped .app would.
            version = "0.0.0-dev+cli"; source = "fallback: no Info.plist in CLI binary"
        }
        return NativeContext(
            osVersion: p.operatingSystemVersionString,
            thermalState: thermal,
            processorCount: p.processorCount,
            physicalMemoryGB: Double(p.physicalMemory) / 1_000_000_000.0,
            uptimeSeconds: Int(p.systemUptime),
            deviceModel: sysctlString("hw.model") ?? "unknown",
            appVersion: version,
            appVersionSource: source,
            processName: p.processName,
            pid: p.processIdentifier
        )
    }

    /// Resource attributes shared by all signals (OTel semconv + macos.*).
    func resourceAttrs(serviceName: String, scenarioId: String, stableInstance: Bool = false) -> [(String, String)] {
        [
            ("service.name", serviceName),
            ("service.version", appVersion),
            ("service.instance.id", stableInstance ? "\(processName)-frozen" : "\(processName)-\(pid)"),
            ("os.type", "darwin"),
            ("os.description", osVersion),
            ("host.arch", sysctlString("hw.machine") ?? "unknown"),
            ("device.model.identifier", deviceModel),
            ("macos.thermal_state", thermalState),
            ("macos.scenario_id", scenarioId),
        ]
    }
}

func sysctlString(_ name: String) -> String? {
    var size = 0
    guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else { return nil }
    var buf = [CChar](repeating: 0, count: size)
    guard sysctlbyname(name, &buf, &size, nil, 0) == 0 else { return nil }
    return String(cString: buf)
}

/// Real stack frames captured in-process (symbolicated while the binary
/// retains symbols; the crash/dSYM path covers the stripped case).
func captureStackTrace(maxChars: Int = 4000) -> String {
    let frames = Thread.callStackSymbols
    var out = frames.joined(separator: "\n")
    if out.count > maxChars {
        out = String(out.prefix(maxChars)) + "\n... (truncated)"
    }
    return out
}

let unifiedLog = Logger(subsystem: "com.tailrocks.macos-playground", category: "harness")

func emitUnifiedLog(scenario: String, traceIdHex: String, marker: String) {
    unifiedLog.info("scenario=\(scenario, privacy: .public) trace_id=\(traceIdHex, privacy: .public) marker=\(marker, privacy: .public)")
    unifiedLog.error("scenario=\(scenario, privacy: .public) emitted error-context marker=\(marker, privacy: .public)")
}

/// Honest MetricKit probe: subscribes and waits. CLI processes without an
/// installed app identity receive no payloads; the probe records that fact
/// instead of faking diagnostics.
final class MetricKitProbe: NSObject, MXMetricManagerSubscriber {
    private var metricCount = 0
    private var diagnosticCount = 0
    private let lock = NSLock()

    func didReceive(_ payloads: [MXMetricPayload]) {
        lock.lock(); metricCount += payloads.count; lock.unlock()
    }

    func didReceive(_ payloads: [MXDiagnosticPayload]) {
        lock.lock(); diagnosticCount += payloads.count; lock.unlock()
    }

    static func run(waitSeconds: UInt32) -> (metrics: Int, diagnostics: Int) {
        let probe = MetricKitProbe()
        MXMetricManager.shared.add(probe)
        sleep(waitSeconds)
        MXMetricManager.shared.remove(probe)
        return (probe.metricCount, probe.diagnosticCount)
    }
}
