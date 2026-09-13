import Darwin
import Foundation
import MachO
import MetricKit
import os

/// Real native context collected from macOS APIs. No stubs: every value comes
/// from ProcessInfo/sysctl/Bundle/Mach-O, and unavailable values are labeled.
struct NativeContext {
    var osVersion: String
    var thermalState: String
    var processorCount: Int
    var physicalMemoryGB: Double
    var memoryResidentBytes: Int
    var uptimeSeconds: Int
    var deviceModel: String
    var appVersion: String
    var appVersionSource: String
    var processName: String
    var pid: Int32
    var buildUUID: String
    var lowPowerMode: Bool

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
            memoryResidentBytes: taskResidentBytes(),
            uptimeSeconds: Int(p.systemUptime),
            deviceModel: sysctlString("hw.model") ?? "unknown",
            appVersion: version,
            appVersionSource: source,
            processName: p.processName,
            pid: p.processIdentifier,
            buildUUID: currentExecutableUUID() ?? "unknown",
            lowPowerMode: p.isLowPowerModeEnabled
        )
    }

    /// Resource attributes shared by all signals (OTel semconv + macos.*).
    func resourceAttrs(serviceName: String, scenarioId: String, stableInstance: Bool = false) -> [(String, String)] {
        let buildIdHex = buildUUID.replacingOccurrences(of: "-", with: "").lowercased()
        return [
            ("service.name", serviceName),
            ("service.version", appVersion),
            ("service.instance.id", stableInstance ? "\(processName)-frozen" : "\(processName)-\(pid)"),
            ("telemetry.sdk.name", "macos-playground"),
            ("telemetry.sdk.language", "swift"),
            ("telemetry.sdk.version", "0.1.0"),
            ("os.type", "darwin"),
            ("os.name", "macOS"),
            ("os.description", osVersion),
            ("host.arch", sysctlString("hw.machine") ?? "unknown"),
            ("device.model.identifier", deviceModel),
            ("process.executable.name", processName),
            ("process.executable.build_id", buildIdHex),
            ("macos.build_uuid", buildUUID),
            ("macos.app_version_source", appVersionSource),
            ("macos.thermal_state", thermalState),
            ("macos.low_power_mode", lowPowerMode ? "true" : "false"),
            ("macos.memory.physical_gb", String(format: "%.1f", physicalMemoryGB)),
            ("macos.memory.resident_bytes", stableInstance ? "frozen" : "\(memoryResidentBytes)"),
            ("macos.cpu.logical_count", "\(processorCount)"),
            ("macos.scenario_id", scenarioId),
        ]
    }
}

func taskResidentBytes() -> Int {
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
    let kr = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
        }
    }
    guard kr == KERN_SUCCESS else { return 0 }
    return Int(info.resident_size)
}

func sysctlString(_ name: String) -> String? {
    var size = 0
    guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0 else { return nil }
    var buf = [CChar](repeating: 0, count: size)
    guard sysctlbyname(name, &buf, &size, nil, 0) == 0 else { return nil }
    return String(cString: buf)
}

/// Mach-O `LC_UUID` of the running image — the dSYM join key.
func currentExecutableUUID() -> String? {
    guard let header = _dyld_get_image_header(0) else { return nil }
    let magic = header.pointee.magic
    let headerSize: Int
    if magic == MH_MAGIC_64 || magic == MH_CIGAM_64 {
        headerSize = MemoryLayout<mach_header_64>.size
    } else {
        headerSize = MemoryLayout<mach_header>.size
    }
    let ncmds = Int(header.pointee.ncmds)
    var p = UnsafeRawPointer(header).advanced(by: headerSize)
    for _ in 0 ..< ncmds {
        let lc = p.load(as: load_command.self)
        if lc.cmd == UInt32(LC_UUID) {
            let uuid = p.assumingMemoryBound(to: uuid_command.self).pointee.uuid
            return UUID(uuid: uuid).uuidString
        }
        p = p.advanced(by: Int(lc.cmdsize))
    }
    return nil
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
