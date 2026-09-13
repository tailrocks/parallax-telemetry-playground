import Foundation

/// Minimal protobuf writer covering exactly the OTLP subset this harness emits.
/// Field numbers follow opentelemetry-proto 1.x:
/// trace.proto, logs.proto, metrics.proto, common.proto, resource.proto.
final class ProtoWriter {
    private(set) var bytes = Data()

    @discardableResult
    func varint(field: Int, _ value: UInt64) -> ProtoWriter {
        tag(field: field, wire: 0)
        var v = value
        repeat {
            var b = UInt8(v & 0x7F)
            v >>= 7
            if v != 0 { b |= 0x80 }
            bytes.append(b)
        } while v != 0
        return self
    }

    @discardableResult
    func fixed64(field: Int, _ value: UInt64) -> ProtoWriter {
        tag(field: field, wire: 1)
        var v = value.littleEndian
        bytes.append(Data(bytes: &v, count: 8))
        return self
    }

    @discardableResult
    func double(field: Int, _ value: Double) -> ProtoWriter {
        fixed64(field: field, value.bitPattern)
    }

    @discardableResult
    func rawBytes(field: Int, _ value: Data) -> ProtoWriter {
        tag(field: field, wire: 2)
        appendLenDelimited(value)
        return self
    }

    @discardableResult
    func string(field: Int, _ value: String) -> ProtoWriter {
        rawBytes(field: field, Data(value.utf8))
    }

    @discardableResult
    func message(field: Int, _ build: (ProtoWriter) -> Void) -> ProtoWriter {
        let w = ProtoWriter()
        build(w)
        return rawBytes(field: field, w.bytes)
    }

    private func tag(field: Int, wire: Int) {
        let t = UInt64(field << 3 | wire)
        var v = t
        repeat {
            var b = UInt8(v & 0x7F)
            v >>= 7
            if v != 0 { b |= 0x80 }
            bytes.append(b)
        } while v != 0
    }

    private func appendLenDelimited(_ value: Data) {
        var v = UInt64(value.count)
        repeat {
            var b = UInt8(v & 0x7F)
            v >>= 7
            if v != 0 { b |= 0x80 }
            bytes.append(b)
        } while v != 0
        bytes.append(value)
    }
}

/// Deterministic PRNG (splitmix64) so `--seed` reproduces identical IDs.
struct SplitMix64 {
    private var state: UInt64
    init(seed: UInt64) { state = seed &+ 0x9E37_79B9_7F4A_7C15 }
    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
    mutating func bytes(count: Int) -> Data {
        guard count > 0 else { return Data() }
        var out = Data()
        while out.count < count {
            var v = next().littleEndian
            out.append(Data(bytes: &v, count: 8))
        }
        var trimmed = out.prefix(count)
        if trimmed.allSatisfy({ $0 == 0 }) {
            trimmed[trimmed.count - 1] = 1 // OTLP forbids all-zero IDs
        }
        return Data(trimmed)
    }
}

func hexString(_ data: Data) -> String {
    data.map { String(format: "%02x", $0) }.joined()
}

func dataFromHex(_ hex: String) -> Data? {
    guard hex.count % 2 == 0 else { return nil }
    var out = Data()
    var i = hex.startIndex
    while i < hex.endIndex {
        let j = hex.index(i, offsetBy: 2)
        guard let b = UInt8(hex[i ..< j], radix: 16) else { return nil }
        out.append(b)
        i = j
    }
    return out
}

// MARK: - OTLP common

func writeIntAttr(into w: ProtoWriter, field: Int, key: String, value: Int64) {
    w.message(field: field) { kv in
        kv.string(field: 1, key)
        kv.message(field: 2) { v in
            v.varint(field: 3 /* AnyValue.int_value */, UInt64(bitPattern: value))
        }
    }
}

func writeStringAttr(into w: ProtoWriter, field: Int, key: String, value: String) {
    w.message(field: field) { kv in
        kv.string(field: 1, key)
        kv.message(field: 2) { v in
            v.string(field: 1, value)
        }
    }
}

func writeResource(_ w: ProtoWriter, attrs: [(String, String)]) {
    w.message(field: 1 /* .resource */) { res in
        for (k, v) in attrs {
            writeStringAttr(into: res, field: 1 /* Resource.attributes */, key: k, value: v)
        }
    }
}

func writeScope(_ w: ProtoWriter, name: String, version: String) {
    w.message(field: 1 /* .scope */) { s in
        s.string(field: 1, name)
        s.string(field: 2, version)
    }
}

// MARK: - OTLP traces

struct OtlpSpan {
    var traceId: Data // 16 bytes
    var spanId: Data // 8 bytes
    var parentSpanId: Data? // 8 bytes
    var name: String
    var kind: Int // SpanKind enum; 1 internal, 3 client
    var startNanos: UInt64
    var endNanos: UInt64
    var stringAttrs: [(String, String)]
    var intAttrs: [(String, Int64)]
    var events: [OtlpEvent]
    var statusCode: Int // 0 unset, 1 ok, 2 error
    var statusMessage: String
}

struct OtlpEvent {
    var timeNanos: UInt64
    var name: String
    var stringAttrs: [(String, String)]
}

func buildTracesData(
    resourceAttrs: [(String, String)],
    scopeName: String,
    scopeVersion: String,
    spans: [OtlpSpan]
) -> Data {
    let w = ProtoWriter()
    w.message(field: 1 /* TracesData.resource_spans */) { rs in
        writeResource(rs, attrs: resourceAttrs)
        rs.message(field: 2 /* ResourceSpans.scope_spans */) { ss in
            writeScope(ss, name: scopeName, version: scopeVersion)
            for s in spans {
                ss.message(field: 2 /* ScopeSpans.spans */) { sp in
                    sp.rawBytes(field: 1, s.traceId)
                    sp.rawBytes(field: 2, s.spanId)
                    if let p = s.parentSpanId { sp.rawBytes(field: 4, p) }
                    sp.string(field: 5, s.name)
                    sp.varint(field: 6, UInt64(s.kind))
                    sp.fixed64(field: 7, s.startNanos)
                    sp.fixed64(field: 8, s.endNanos)
                    for (k, v) in s.stringAttrs {
                        writeStringAttr(into: sp, field: 9, key: k, value: v)
                    }
                    for (k, v) in s.intAttrs {
                        writeIntAttr(into: sp, field: 9, key: k, value: v)
                    }
                    for e in s.events {
                        sp.message(field: 11) { ev in
                            ev.fixed64(field: 1, e.timeNanos)
                            ev.string(field: 2, e.name)
                            for (k, v) in e.stringAttrs {
                                writeStringAttr(into: ev, field: 3, key: k, value: v)
                            }
                        }
                    }
                    if s.statusCode != 0 || !s.statusMessage.isEmpty {
                        sp.message(field: 15) { st in
                            if !s.statusMessage.isEmpty { st.string(field: 2, s.statusMessage) }
                            st.varint(field: 3, UInt64(s.statusCode))
                        }
                    }
                }
            }
        }
    }
    return w.bytes
}

// MARK: - OTLP logs

struct OtlpLog {
    var timeNanos: UInt64
    var observedNanos: UInt64
    var severityNumber: Int // 9 info, 13 warn, 17 error
    var severityText: String
    var body: String
    var stringAttrs: [(String, String)]
    var traceId: Data?
    var spanId: Data?
}

func buildLogsData(
    resourceAttrs: [(String, String)],
    scopeName: String,
    scopeVersion: String,
    logs: [OtlpLog]
) -> Data {
    let w = ProtoWriter()
    w.message(field: 1 /* LogsData.resource_logs */) { rl in
        writeResource(rl, attrs: resourceAttrs)
        rl.message(field: 2 /* ResourceLogs.scope_logs */) { sl in
            writeScope(sl, name: scopeName, version: scopeVersion)
            for l in logs {
                sl.message(field: 2 /* ScopeLogs.log_records */) { lr in
                    lr.fixed64(field: 1, l.timeNanos)
                    lr.varint(field: 2, UInt64(l.severityNumber))
                    lr.string(field: 3, l.severityText)
                    lr.message(field: 5 /* body */) { b in
                        b.string(field: 1, l.body)
                    }
                    for (k, v) in l.stringAttrs {
                        writeStringAttr(into: lr, field: 6, key: k, value: v)
                    }
                    if let t = l.traceId { lr.rawBytes(field: 9, t) }
                    if let s = l.spanId { lr.rawBytes(field: 10, s) }
                    lr.fixed64(field: 11, l.observedNanos)
                }
            }
        }
    }
    return w.bytes
}

// MARK: - OTLP metrics

struct OtlpSumPoint {
    var timeNanos: UInt64
    var startNanos: UInt64
    var value: Double
    var stringAttrs: [(String, String)]
}

struct OtlpHistogramPoint {
    var timeNanos: UInt64
    var startNanos: UInt64
    var count: UInt64
    var sum: Double
    var bounds: [Double]
    var bucketCounts: [UInt64]
    var stringAttrs: [(String, String)]
}

func buildMetricsData(
    resourceAttrs: [(String, String)],
    scopeName: String,
    scopeVersion: String,
    sums: [(name: String, description: String, unit: String, monotonic: Bool, points: [OtlpSumPoint])],
    histograms: [(name: String, description: String, unit: String, points: [OtlpHistogramPoint])]
) -> Data {
    let w = ProtoWriter()
    w.message(field: 1 /* MetricsData.resource_metrics */) { rm in
        writeResource(rm, attrs: resourceAttrs)
        rm.message(field: 2 /* ResourceMetrics.scope_metrics */) { sm in
            writeScope(sm, name: scopeName, version: scopeVersion)
            for s in sums {
                sm.message(field: 2 /* ScopeMetrics.metrics */) { m in
                    m.string(field: 1, s.name)
                    m.string(field: 2, s.description)
                    m.string(field: 3, s.unit)
                    m.message(field: 7 /* Metric.sum */) { sum in
                        for p in s.points {
                            sum.message(field: 1 /* Sum.data_points */) { dp in
                                dp.fixed64(field: 2, p.startNanos)
                                dp.fixed64(field: 3, p.timeNanos)
                                dp.double(field: 4 /* as_double */, p.value)
                                for (k, v) in p.stringAttrs {
                                    writeStringAttr(into: dp, field: 7, key: k, value: v)
                                }
                            }
                        }
                        sum.varint(field: 2 /* aggregation_temporality */, 2 /* CUMULATIVE */)
                        sum.varint(field: 3 /* is_monotonic */, s.monotonic ? 1 : 0)
                    }
                }
            }
            for h in histograms {
                sm.message(field: 2) { m in
                    m.string(field: 1, h.name)
                    m.string(field: 2, h.description)
                    m.string(field: 3, h.unit)
                    m.message(field: 9 /* Metric.histogram */) { hist in
                        for p in h.points {
                            hist.message(field: 1 /* Histogram.data_points */) { dp in
                                dp.fixed64(field: 2, p.startNanos)
                                dp.fixed64(field: 3, p.timeNanos)
                                dp.fixed64(field: 4, p.count)
                                dp.double(field: 5, p.sum)
                                for c in p.bucketCounts { dp.fixed64(field: 6, c) }
                                for b in p.bounds { dp.double(field: 7, b) }
                                for (k, v) in p.stringAttrs {
                                    writeStringAttr(into: dp, field: 9, key: k, value: v)
                                }
                            }
                        }
                        hist.varint(field: 2 /* aggregation_temporality */, 2 /* CUMULATIVE */)
                    }
                }
            }
        }
    }
    return w.bytes
}
