//! Hand-built OTLP corner-case shapes (plan 161): structures no real service
//! flow can mint (multi-root traces, negative clock skew, zero-duration
//! spans, 500-span fan-outs, cross-trace links, oversized names, event
//! floods, log/metric edge shapes). Emitted as `service.name =
//! playground-shapes` over OTLP/HTTP protobuf so counts and structure are
//! exact and unit-testable.

use anyhow::Context as _;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as AnyValueEnum;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::{
    Gauge, Histogram, HistogramDataPoint, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics,
    Sum, metric::Data, number_data_point,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::{Event, Link};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use prost::Message;

use playground_telemetry::invocation;
use playground_telemetry::semconv;

pub(crate) const SERVICE: &str = "playground-shapes";

pub(crate) fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(AnyValueEnum::StringValue(value.to_string())),
        }),
        key_strindex: 0,
    }
}

fn kv_int(key: &str, value: i64) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(AnyValueEnum::IntValue(value)),
        }),
        key_strindex: 0,
    }
}

fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos() as u64
}

fn id16(seed: u64) -> Vec<u8> {
    let mut bytes = uuid::Uuid::new_v4().into_bytes().to_vec();
    bytes[8..16].copy_from_slice(&seed.to_be_bytes());
    bytes
}

fn id8(seed: u64) -> Vec<u8> {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&uuid::Uuid::new_v4().as_bytes()[..8]);
    let mut out = bytes.to_vec();
    out[..8].copy_from_slice(&(seed ^ u64::from_be_bytes(bytes)).to_be_bytes());
    out
}

pub(crate) struct SpanSpec {
    pub trace: Vec<u8>,
    pub id: Vec<u8>,
    pub parent: Option<Vec<u8>>,
    pub name: String,
    pub kind: i32,
    pub start: u64,
    pub end: u64,
    pub error: bool,
    pub attrs: Vec<KeyValue>,
    pub events: Vec<Event>,
    pub links: Vec<Link>,
    /// Override the exporting service (`None` = the shared shapes service).
    /// Clock-skew detection is cross-service by definition, so skew shapes
    /// must place parent and child on different services.
    pub service: Option<String>,
}

impl SpanSpec {
    fn basic(trace: &[u8], seed: u64, parent: Option<Vec<u8>>, name: &str, start: u64) -> Self {
        Self {
            trace: trace.to_vec(),
            id: id8(seed),
            parent,
            name: name.to_string(),
            kind: 1,
            start,
            end: start + 5_000_000,
            error: false,
            attrs: Vec::new(),
            events: Vec::new(),
            links: Vec::new(),
            service: None,
        }
    }
}

pub(crate) fn traces_request(spans: Vec<SpanSpec>) -> ExportTraceServiceRequest {
    let invocation_id = invocation::invocation_id();
    // Group by exporting service so per-span overrides land under their own
    // resource, preserving order within each group.
    let mut groups: Vec<(String, Vec<Span>)> = Vec::new();
    for spec in spans {
        let service = spec.service.as_deref().unwrap_or(SERVICE).to_string();
        let span = Span {
            trace_id: spec.trace,
            span_id: spec.id,
            parent_span_id: spec.parent.unwrap_or_default(),
            name: spec.name,
            kind: spec.kind,
            start_time_unix_nano: spec.start,
            end_time_unix_nano: spec.end,
            attributes: spec.attrs,
            events: spec.events,
            links: spec.links,
            status: spec.error.then(|| Status {
                code: 2,
                message: "shape error".into(),
            }),
            ..Default::default()
        };
        match groups.iter_mut().find(|(name, _)| *name == service) {
            Some((_, list)) => list.push(span),
            None => groups.push((service, vec![span])),
        }
    }
    ExportTraceServiceRequest {
        resource_spans: groups
            .into_iter()
            .map(|(service, spans)| ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![
                        kv(semconv::SERVICE_NAME, &service),
                        kv(semconv::CLI_INVOCATION_ID, invocation_id),
                    ],
                    ..Default::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .collect(),
    }
}

/// t-deep: one linear chain, depth ≥ 12, alternating simulated services in
/// the span names (single resource keeps the generator honest about shape).
pub(crate) fn t_deep() -> Vec<SpanSpec> {
    let trace = id16(1);
    let base = now_nanos();
    let mut spans = Vec::new();
    let mut parent: Option<Vec<u8>> = None;
    for depth in 0..14u64 {
        let service = ["gateway", "orders", "billing"][(depth % 3) as usize];
        let mut span = SpanSpec::basic(
            &trace,
            depth + 1,
            parent.clone(),
            &format!("{service}.step_{depth}"),
            base + depth * 1_000_000,
        );
        span.end = base + (14 - depth) * 1_000_000 + 14_000_000;
        parent = Some(span.id.clone());
        spans.push(span);
    }
    spans
}

/// t-wide: one trace with ≥ 500 spans fanning out from one root.
pub(crate) fn t_wide() -> Vec<SpanSpec> {
    let trace = id16(2);
    let base = now_nanos();
    let root = SpanSpec::basic(&trace, 1, None, "fanout.root", base);
    let root_id = root.id.clone();
    let mut spans = vec![root];
    for index in 0..520u64 {
        spans.push(SpanSpec::basic(
            &trace,
            index + 2,
            Some(root_id.clone()),
            &format!("fanout.child_{index}"),
            base + index * 10_000,
        ));
    }
    spans
}

/// t-multiroot: one trace id containing two root spans (legal OTel).
pub(crate) fn t_multiroot() -> Vec<SpanSpec> {
    let trace = id16(3);
    let base = now_nanos();
    vec![
        SpanSpec::basic(&trace, 1, None, "root.alpha", base),
        SpanSpec::basic(&trace, 2, None, "root.beta", base + 2_000_000),
    ]
}

/// t-orphan: a child whose parent span id never arrives.
pub(crate) fn t_orphan() -> Vec<SpanSpec> {
    let trace = id16(4);
    let base = now_nanos();
    let root = SpanSpec::basic(&trace, 1, None, "orphan.root", base);
    let mut orphan = SpanSpec::basic(
        &trace,
        2,
        Some(id8(999)), // never exported
        "orphan.detached_child",
        base + 1_000_000,
    );
    orphan.error = true;
    vec![root, orphan]
}

/// t-skew: a SERVER child on a second service that starts before its CLIENT
/// parent. Cross-service placement is essential: skew detection thresholds
/// same-service drift at minutes (normal scheduler jitter) but cross-service
/// drift at 50 ms, and real clock skew only exists across hosts. 120 ms
/// backdate crosses the 50 ms warning threshold.
pub(crate) fn t_skew() -> Vec<SpanSpec> {
    let trace = id16(5);
    let base = now_nanos();
    let mut parent = SpanSpec::basic(&trace, 1, None, "skew.client_call", base);
    parent.kind = 3;
    parent.end = base + 200_000_000;
    let mut child = SpanSpec::basic(
        &trace,
        2,
        Some(parent.id.clone()),
        "skew.server_handle",
        base - 120_000_000, // starts BEFORE the parent (clock skew)
    );
    child.kind = 2;
    child.end = base + 5_000_000;
    child.service = Some(format!("{SERVICE}-remote"));
    vec![parent, child]
}

/// t-zero: zero-duration spans and identical start/end at µs resolution.
pub(crate) fn t_zero() -> Vec<SpanSpec> {
    let trace = id16(6);
    let base = now_nanos();
    let root = SpanSpec::basic(&trace, 1, None, "zero.root", base);
    let root_id = root.id.clone();
    let mut zero = SpanSpec::basic(
        &trace,
        2,
        Some(root_id.clone()),
        "zero.instant",
        base + 1_000,
    );
    zero.end = zero.start;
    let mut micro = SpanSpec::basic(
        &trace,
        3,
        Some(root_id),
        "zero.microsecond_twin",
        base + 2_000,
    );
    micro.end = micro.start + 1_000; // 1µs
    vec![root, zero, micro]
}

/// t-links: two traces cross-linked in both directions.
pub(crate) fn t_links() -> Vec<SpanSpec> {
    let trace_a = id16(7);
    let trace_b = id16(8);
    let base = now_nanos();
    let mut a = SpanSpec::basic(&trace_a, 1, None, "links.origin", base);
    let mut b = SpanSpec::basic(&trace_b, 2, None, "links.target", base + 4_000_000);
    a.links.push(Link {
        trace_id: trace_b.clone(),
        span_id: b.id.clone(),
        ..Default::default()
    });
    b.links.push(Link {
        trace_id: trace_a.clone(),
        span_id: a.id.clone(),
        ..Default::default()
    });
    vec![a, b]
}

/// t-longnames: names/attribute values at 1-4 KiB with unicode + emoji.
pub(crate) fn t_longnames() -> Vec<SpanSpec> {
    let trace = id16(9);
    let base = now_nanos();
    let long_name = format!("long.name.🛰️.{}", "セグメント/".repeat(96));
    let mut span = SpanSpec::basic(&trace, 1, None, &long_name, base);
    span.attrs.push(kv(
        "shape.long_value",
        &format!("v-🧪-{}", "payload-Ünïcode-".repeat(240)),
    ));
    span.attrs
        .push(kv(&format!("shape.long_key.{}", "k".repeat(160)), "short"));
    vec![span]
}

/// t-events: ≥ 50 span events incl. Rust / Java / browser stacktraces.
pub(crate) fn t_events() -> Vec<SpanSpec> {
    let trace = id16(10);
    let base = now_nanos();
    let mut span = SpanSpec::basic(&trace, 1, None, "events.flood", base);
    span.error = true;
    for index in 0..48u64 {
        span.events.push(Event {
            time_unix_nano: base + index * 50_000,
            name: format!("progress.tick_{index}"),
            attributes: vec![kv_int("tick", index as i64)],
            ..Default::default()
        });
    }
    for (language, error_type, stack) in exception_shapes() {
        span.events.push(Event {
            time_unix_nano: base + 3_000_000,
            name: "exception".to_string(),
            attributes: vec![
                kv("exception.type", error_type),
                kv("exception.message", &format!("{language} failure")),
                kv("exception.stacktrace", stack),
            ],
            ..Default::default()
        });
    }
    vec![span]
}

pub(crate) fn exception_shapes() -> [(&'static str, &'static str, &'static str); 3] {
    [
        (
            "rust",
            "shapes::CheckoutError",
            "Error: checkout failed\n\nCaused by:\n    0: reserving inventory\n    1: connection reset by peer\n\nStack backtrace:\n   0: shapes::checkout\n             at ./src/checkout.rs:42:13\n   1: tokio::runtime::task::core::Core<T,S>::poll",
        ),
        (
            "java",
            "java.lang.IllegalStateException",
            "java.lang.IllegalStateException: checkout failed\n\tat dev.tailrocks.shapes.Checkout.submit(Checkout.java:42)\n\tat java.base/java.util.concurrent.FutureTask.run(FutureTask.java:317)\nCaused by: java.net.SocketException: Connection reset\n\tat java.base/sun.nio.ch.NioSocketImpl.implRead(NioSocketImpl.java:314)",
        ),
        (
            "browser",
            "TypeError",
            "TypeError: Cannot read properties of undefined (reading 'total')\n    at submitCheckout (https://shop.example/assets/checkout-9f2.js:1:8214)\n    at HTMLFormElement.onSubmit (https://shop.example/assets/app-77a.js:2:1911)",
        ),
    ]
}

/// l-burst: `count` logs across severities in a tight window.
pub(crate) fn l_burst(count: usize) -> ExportLogsServiceRequest {
    let base = now_nanos();
    let records = (0..count)
        .map(|index| {
            let severity = [5, 9, 13, 17, 21][index % 5];
            log_record(
                base + (index as u64) * 5_000,
                severity,
                &format!("burst log line {index} lane={}", index % 7),
                vec![kv_int("burst.index", index as i64)],
            )
        })
        .collect();
    logs_request(records)
}

/// l-bodies: structured JSON, 32 KiB body, ANSI escapes, blank body, and an
/// identical-timestamp run.
pub(crate) fn l_bodies() -> ExportLogsServiceRequest {
    let base = now_nanos();
    let mut records = vec![
        log_record(
            base,
            9,
            r#"{"event":"order.placed","order":{"id":"o-1","total":41.5,"items":[{"sku":"WIDGET-1","qty":3}]}}"#,
            vec![kv("shape.body", "json")],
        ),
        log_record(
            base + 1_000,
            13,
            &format!("oversized body: {}", "x".repeat(32 * 1024)),
            vec![kv("shape.body", "oversized")],
        ),
        log_record(
            base + 2_000,
            17,
            "\u{1b}[31merror\u{1b}[0m with \u{1b}[1mANSI\u{1b}[0m escapes",
            vec![kv("shape.body", "ansi")],
        ),
        log_record(base + 3_000, 9, "", vec![kv("shape.body", "blank")]),
    ];
    for run in 0..5 {
        records.push(log_record(
            base + 4_000,
            9,
            &format!("identical timestamp run entry {run}"),
            vec![kv("shape.body", "same-ts"), kv_int("run", run)],
        ));
    }
    logs_request(records)
}

fn log_record(ts: u64, severity: i32, body: &str, mut attrs: Vec<KeyValue>) -> LogRecord {
    attrs.push(kv(semconv::CLI_INVOCATION_ID, invocation::invocation_id()));
    LogRecord {
        time_unix_nano: ts,
        observed_time_unix_nano: ts,
        severity_number: severity,
        severity_text: match severity {
            21.. => "FATAL",
            17.. => "ERROR",
            13.. => "WARN",
            9.. => "INFO",
            _ => "DEBUG",
        }
        .to_string(),
        body: Some(AnyValue {
            value: Some(AnyValueEnum::StringValue(body.to_string())),
        }),
        attributes: attrs,
        ..Default::default()
    }
}

fn logs_request(records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv(semconv::SERVICE_NAME, SERVICE)],
                ..Default::default()
            }),
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// m-shapes anchor trace: the span the histogram exemplar deep-links to. An
/// exemplar pointing at a trace that was never exported cannot prove the
/// deep-link works — the target page would 404.
pub(crate) fn m_shapes_anchor() -> Vec<SpanSpec> {
    let trace = id16(42);
    let base = now_nanos();
    let mut span = SpanSpec::basic(&trace, 42, None, "shapes.exemplar_anchor", base);
    span.kind = 2;
    vec![span]
}

/// m-shapes: counter reset mid-window, gauge with gaps, explicit histogram
/// with exemplar-bearing buckets. The histogram uses the standard
/// `http.server.request.duration` name so the service latency panel — the
/// exemplar-rendering surface — picks it up; the exemplar references the
/// anchor trace exported alongside.
pub(crate) fn m_shapes(anchor: &[SpanSpec]) -> ExportMetricsServiceRequest {
    let anchor_span = anchor.first().expect("anchor span");
    // Points step 5 minutes back into the past: dashboard charts bucket by
    // minutes, so a sub-minute gap (or future timestamps) can never render.
    let step = 300_000_000_000u64; // 5 min
    let base = now_nanos() - 3 * step;
    let number = |ts: u64, value: f64| NumberDataPoint {
        time_unix_nano: ts,
        start_time_unix_nano: base,
        value: Some(number_data_point::Value::AsDouble(value)),
        ..Default::default()
    };
    let counter = Metric {
        name: "shapes.requests.total".to_string(),
        data: Some(Data::Sum(Sum {
            data_points: vec![
                number(base, 100.0),
                number(base + step, 180.0),
                number(base + 2 * step, 12.0), // reset mid-window
                number(base + 3 * step, 60.0),
            ],
            aggregation_temporality: 2,
            is_monotonic: true,
        })),
        ..Default::default()
    };
    let gauge = Metric {
        name: "shapes.queue.depth".to_string(),
        data: Some(Data::Gauge(Gauge {
            data_points: vec![
                number(base, 4.0),
                // deliberate gap: nothing for two steps
                number(base + 3 * step, 9.0),
            ],
        })),
        ..Default::default()
    };
    let histogram = Metric {
        name: semconv::HTTP_SERVER_REQUEST_DURATION.to_string(),
        data: Some(Data::Histogram(Histogram {
            data_points: vec![HistogramDataPoint {
                time_unix_nano: base + step,
                start_time_unix_nano: base,
                count: 7,
                sum: Some(2.1),
                bucket_counts: vec![2, 3, 2],
                explicit_bounds: vec![0.1, 0.5],
                exemplars: vec![opentelemetry_proto::tonic::metrics::v1::Exemplar {
                    time_unix_nano: base + step / 2,
                    trace_id: anchor_span.trace.clone(),
                    span_id: anchor_span.id.clone(),
                    value: Some(
                        opentelemetry_proto::tonic::metrics::v1::exemplar::Value::AsDouble(0.42),
                    ),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            aggregation_temporality: 2,
        })),
        ..Default::default()
    };
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv(semconv::SERVICE_NAME, SERVICE)],
                ..Default::default()
            }),
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![counter, gauge, histogram],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// l-patterns (plan 165): ≥20k log lines drawn from 12 stable templates with
/// per-line parameter churn (ids, ips, durations) plus one "spiking"
/// template concentrated late in the window, so Drain clustering quality and
/// the spike ranking are exactly assertable: 11 steady templates × 1,200
/// lines + 6,800 spike lines = 20,000.
pub(crate) const L_PATTERNS_STEADY_TEMPLATES: usize = 11;
pub(crate) const L_PATTERNS_STEADY_LINES: u64 = 1_200;
pub(crate) const L_PATTERNS_SPIKE_LINES: u64 = 6_800;

fn l_patterns_line(template: usize, index: u64) -> (String, i32) {
    let uid = 10_000 + (index % 977);
    let ip = format!("10.{}.{}.{}", index % 8, (index / 8) % 250, index % 250);
    let ms = 3 + (index % 512);
    let order = format!("o-{}", 42_000 + index % 5_003);
    let sku = format!("SKU-{}", index % 61);
    match template {
        0 => (format!("user {uid} logged in from {ip}"), 9),
        1 => (format!("GET /api/orders/{order} completed in {ms}ms"), 9),
        2 => (format!("cache miss for key products:{sku}"), 5),
        3 => (format!("published event order.updated id={order}"), 9),
        4 => (format!("db query took {ms}ms rows={}", index % 40), 5),
        5 => (
            format!(
                "retrying payment for order {order} attempt {}",
                1 + index % 3
            ),
            13,
        ),
        6 => (format!("session {uid} expired, refreshing token"), 9),
        7 => (format!("inventory reserve {sku} qty={}", 1 + index % 9), 9),
        8 => (format!("rate limit near threshold for {ip}"), 13),
        9 => (format!("email queued to user-{uid}@example.com"), 9),
        10 => (
            format!("feature flag checkout_v2 evaluated for user {uid}"),
            5,
        ),
        _ => (
            format!("connection reset by peer {ip} while streaming order {order}"),
            17,
        ),
    }
}

pub(crate) fn l_patterns() -> Vec<LogRecord> {
    let base = now_nanos().saturating_sub(300_000_000_000); // 5-minute window
    let mut records = Vec::new();
    for template in 0..L_PATTERNS_STEADY_TEMPLATES {
        for index in 0..L_PATTERNS_STEADY_LINES {
            let (body, severity) = l_patterns_line(template, index);
            records.push(log_record(
                // Steady templates spread evenly across the whole window.
                base + index * 250_000_000 + template as u64 * 1_000,
                severity,
                &body,
                vec![
                    kv("shape.case", "l-patterns"),
                    kv("shape.template", &format!("steady-{template}")),
                ],
            ));
        }
    }
    for index in 0..L_PATTERNS_SPIKE_LINES {
        let (body, severity) = l_patterns_line(usize::MAX, index);
        records.push(log_record(
            // Spike template concentrated in the last fifth of the window.
            base + 240_000_000_000 + index * 8_000_000,
            severity,
            &body,
            vec![
                kv("shape.case", "l-patterns"),
                kv("shape.template", "spike"),
            ],
        ));
    }
    records
}

/// f-attrs (plan 164): spans and logs carrying a documented attribute set
/// with known value distributions — `http.request.method` split exactly
/// 70/20/10 GET/POST/DELETE across 100 spans and 100 logs — so facet counts
/// and where-clause narrowing are exactly assertable.
pub(crate) fn f_attrs_method(index: u64) -> &'static str {
    match index {
        0..=69 => "GET",
        70..=89 => "POST",
        _ => "DELETE",
    }
}

pub(crate) fn f_attrs_spans() -> Vec<SpanSpec> {
    let base = now_nanos();
    (0..100u64)
        .map(|index| {
            let trace = id16(7_000 + index);
            let mut span = SpanSpec::basic(
                &trace,
                7_000 + index,
                None,
                "shapes.facet_request",
                base + index * 3_000_000,
            );
            span.kind = 2;
            span.attrs = vec![
                kv(semconv::HTTP_REQUEST_METHOD, f_attrs_method(index)),
                kv("shape.case", "f-attrs"),
            ];
            span
        })
        .collect()
}

pub(crate) fn f_attrs_logs() -> ExportLogsServiceRequest {
    let base = now_nanos();
    let records = (0..100u64)
        .map(|index| {
            log_record(
                base + index * 3_000,
                9,
                &format!("facet corpus request {index}"),
                vec![
                    kv(semconv::HTTP_REQUEST_METHOD, f_attrs_method(index)),
                    kv("shape.case", "f-attrs"),
                ],
            )
        })
        .collect();
    logs_request(records)
}

/// m-labels (plan 168): one gauge and one monotonic sum emitted with a
/// 3-value `region` label (eu/us/ap) at fixed per-region values, so group-by
/// breakdown output is exactly assertable.
pub(crate) fn m_labels() -> ExportMetricsServiceRequest {
    let step = 300_000_000_000u64; // 5 min, matching m-shapes bucketing
    let base = now_nanos() - 3 * step;
    // Fixed per-region magnitudes: eu=60, us=30, ap=10 (a 6/3/1 split).
    let regions: [(&str, f64); 3] = [("eu", 60.0), ("us", 30.0), ("ap", 10.0)];
    let point = |ts: u64, value: f64, region: &str| NumberDataPoint {
        time_unix_nano: ts,
        start_time_unix_nano: base,
        value: Some(number_data_point::Value::AsDouble(value)),
        attributes: vec![kv("region", region)],
        ..Default::default()
    };
    let gauge_points = (0..=3u64)
        .flat_map(|step_index| {
            regions.map(|(region, magnitude)| point(base + step_index * step, magnitude, region))
        })
        .collect();
    let sum_points = (0..=3u64)
        .flat_map(|step_index| {
            regions.map(|(region, magnitude)| {
                // Monotonic: each region grows by its magnitude every step.
                point(
                    base + step_index * step,
                    magnitude * (step_index + 1) as f64,
                    region,
                )
            })
        })
        .collect();
    let gauge = Metric {
        name: "shapes.region.load".to_string(),
        data: Some(Data::Gauge(Gauge {
            data_points: gauge_points,
        })),
        ..Default::default()
    };
    let sum = Metric {
        name: "shapes.region.requests_total".to_string(),
        data: Some(Data::Sum(Sum {
            data_points: sum_points,
            aggregation_temporality: 2,
            is_monotonic: true,
        })),
        ..Default::default()
    };
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![kv(semconv::SERVICE_NAME, SERVICE)],
                ..Default::default()
            }),
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![gauge, sum],
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// e-burst: one error type repeated for grouping + five distinct
/// `error.type` values under one invocation.
pub(crate) fn e_burst() -> Vec<SpanSpec> {
    let base = now_nanos();
    let mut spans = Vec::new();
    for index in 0..100u64 {
        let trace = id16(1000 + index);
        let mut span = SpanSpec::basic(
            &trace,
            1,
            None,
            "burst.recurring_failure",
            base + index * 2_000_000,
        );
        span.error = true;
        span.attrs
            .push(kv(semconv::ERROR_TYPE, "shapes::RecurringFailure"));
        spans.push(span);
    }
    for (index, error_type) in [
        "shapes::Alpha",
        "shapes::Beta",
        "shapes::Gamma",
        "shapes::Delta",
        "shapes::Epsilon",
    ]
    .iter()
    .enumerate()
    {
        let trace = id16(2000 + index as u64);
        let mut span = SpanSpec::basic(
            &trace,
            1,
            None,
            "burst.distinct_failure",
            base + index as u64 * 3_000_000,
        );
        span.error = true;
        span.attrs.push(kv(semconv::ERROR_TYPE, error_type));
        spans.push(span);
    }
    spans
}

/// e-multi-lang: the same logical failure with Rust/Java/browser fingerprints.
pub(crate) fn e_multi_lang() -> ExportLogsServiceRequest {
    let base = now_nanos();
    let records = exception_shapes()
        .into_iter()
        .enumerate()
        .map(|(index, (language, error_type, stack))| {
            log_record(
                base + index as u64 * 1_000_000,
                17,
                &format!("checkout failed in the {language} tier"),
                vec![
                    kv("exception.type", error_type),
                    kv(
                        "exception.message",
                        &format!("checkout failed ({language})"),
                    ),
                    kv("exception.stacktrace", stack),
                    kv("shape.language", language),
                ],
            )
        })
        .collect();
    logs_request(records)
}

fn otlp_http_base() -> String {
    // OTEL_EXPORTER_OTLP_ENDPOINT is the gRPC endpoint in this repo's
    // compose; the HTTP listener sits on the conventional 4318 next to it.
    if let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_HTTP_ENDPOINT") {
        return endpoint.trim_end_matches('/').to_string();
    }
    let grpc = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:4317".to_string());
    grpc.trim_end_matches('/').replace(":4317", ":4318")
}

async fn post(path: &str, body: Vec<u8>) -> anyhow::Result<()> {
    let url = format!("{}/{path}", otlp_http_base());
    let response = reqwest::Client::new()
        .post(&url)
        .header("content-type", "application/x-protobuf")
        .body(body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "POST {url}: {}",
        response.status()
    );
    Ok(())
}

pub(crate) async fn run(args: Vec<String>) -> anyhow::Result<i32> {
    let Some(id) = args.first().map(String::as_str) else {
        anyhow::bail!(
            "usage: playground shapes <t-deep|t-wide|t-multiroot|t-orphan|t-skew|t-zero|t-links|t-longnames|t-events|l-burst|l-bodies|l-patterns|m-shapes|m-labels|f-attrs|e-burst|e-multi-lang>"
        );
    };
    println!("shapes: emitting {id} as {SERVICE}");
    match id {
        "t-deep" => post_traces(t_deep()).await?,
        "t-wide" => post_traces(t_wide()).await?,
        "t-multiroot" => post_traces(t_multiroot()).await?,
        "t-orphan" => post_traces(t_orphan()).await?,
        "t-skew" => post_traces(t_skew()).await?,
        "t-zero" => post_traces(t_zero()).await?,
        "t-links" => post_traces(t_links()).await?,
        "t-longnames" => post_traces(t_longnames()).await?,
        "t-events" => post_traces(t_events()).await?,
        "l-burst" => post("v1/logs", l_burst(5_000).encode_to_vec()).await?,
        "l-bodies" => post("v1/logs", l_bodies().encode_to_vec()).await?,
        "m-shapes" => {
            let anchor = m_shapes_anchor();
            let metrics = m_shapes(&anchor);
            post_traces(anchor).await?;
            post("v1/metrics", metrics.encode_to_vec()).await?;
        }
        "l-patterns" => {
            // 20k records in 4 posts keeps each request comfortably sized.
            let records = l_patterns();
            for chunk in records.chunks(5_000) {
                post("v1/logs", logs_request(chunk.to_vec()).encode_to_vec()).await?;
            }
        }
        "f-attrs" => {
            post_traces(f_attrs_spans()).await?;
            post("v1/logs", f_attrs_logs().encode_to_vec()).await?;
        }
        "m-labels" => post("v1/metrics", m_labels().encode_to_vec()).await?,
        "e-burst" => post_traces(e_burst()).await?,
        "e-multi-lang" => post("v1/logs", e_multi_lang().encode_to_vec()).await?,
        other => anyhow::bail!("unknown shape id: {other}"),
    }
    println!(
        "shapes: {id} emitted (invocation {})",
        invocation::invocation_id()
    );
    Ok(0)
}

async fn post_traces(spans: Vec<SpanSpec>) -> anyhow::Result<()> {
    let request = traces_request(spans);
    let trace_ids: std::collections::BTreeSet<String> = request
        .resource_spans
        .iter()
        .flat_map(|rs| rs.scope_spans.iter())
        .flat_map(|ss| ss.spans.iter())
        .map(|span| hex(&span.trace_id))
        .collect();
    post("v1/traces", request.encode_to_vec()).await?;
    for trace_id in trace_ids {
        println!("shapes: trace {trace_id}");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(spans: &[SpanSpec]) -> usize {
        spans.iter().filter(|span| span.parent.is_none()).count()
    }

    #[test]
    fn l_patterns_is_twenty_thousand_lines_with_a_late_spike() {
        let records = l_patterns();
        assert_eq!(records.len(), 20_000);
        let template_of = |record: &LogRecord| {
            record
                .attributes
                .iter()
                .find(|attr| attr.key == "shape.template")
                .and_then(|attr| attr.value.as_ref())
                .and_then(|value| match &value.value {
                    Some(AnyValueEnum::StringValue(text)) => Some(text.clone()),
                    _ => None,
                })
                .expect("template attr")
        };
        let spike: Vec<&LogRecord> = records
            .iter()
            .filter(|record| template_of(record) == "spike")
            .collect();
        assert_eq!(spike.len() as u64, L_PATTERNS_SPIKE_LINES);
        let templates: std::collections::BTreeSet<String> =
            records.iter().map(template_of).collect();
        assert_eq!(templates.len(), L_PATTERNS_STEADY_TEMPLATES + 1);
        // Spike lives strictly in the last fifth of the window.
        let min_steady = records
            .iter()
            .filter(|record| template_of(record) != "spike")
            .map(|record| record.time_unix_nano)
            .min()
            .expect("steady records");
        let min_spike = spike
            .iter()
            .map(|record| record.time_unix_nano)
            .min()
            .expect("spike records");
        assert!(min_spike >= min_steady + 240_000_000_000);
        // Parameter churn: same template, different rendered bodies.
        let first_bodies: std::collections::BTreeSet<String> = records
            .iter()
            .filter(|record| template_of(record) == "steady-0")
            .take(50)
            .filter_map(|record| match &record.body.as_ref()?.value {
                Some(AnyValueEnum::StringValue(text)) => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(first_bodies.len() > 40);
    }

    #[test]
    fn f_attrs_method_split_is_seventy_twenty_ten() {
        let spans = f_attrs_spans();
        assert_eq!(spans.len(), 100);
        let count = |method: &str| {
            spans
                .iter()
                .filter(|span| {
                    span.attrs.iter().any(|attr| {
                        attr.key == semconv::HTTP_REQUEST_METHOD
                            && attr.value.as_ref().is_some_and(|value| {
                                value.value == Some(AnyValueEnum::StringValue(method.to_string()))
                            })
                    })
                })
                .count()
        };
        assert_eq!(count("GET"), 70);
        assert_eq!(count("POST"), 20);
        assert_eq!(count("DELETE"), 10);
        let logs = f_attrs_logs();
        let records = &logs.resource_logs[0].scope_logs[0].log_records;
        assert_eq!(records.len(), 100);
    }

    #[test]
    fn m_labels_regions_split_six_three_one() {
        let request = m_labels();
        let metrics = &request.resource_metrics[0].scope_metrics[0].metrics;
        assert_eq!(metrics.len(), 2);
        let Some(Data::Gauge(gauge)) = &metrics[0].data else {
            panic!("first metric must be the gauge");
        };
        // 4 timestamps × 3 regions.
        assert_eq!(gauge.data_points.len(), 12);
        let Some(Data::Sum(sum)) = &metrics[1].data else {
            panic!("second metric must be the sum");
        };
        assert!(sum.is_monotonic);
        assert_eq!(sum.data_points.len(), 12);
        // Monotonic within each region: last eu point is 4 × 60.
        let eu_last = sum
            .data_points
            .iter()
            .rfind(|point| {
                point.attributes.iter().any(|attr| {
                    attr.key == "region"
                        && attr.value.as_ref().is_some_and(|value| {
                            value.value == Some(AnyValueEnum::StringValue("eu".to_string()))
                        })
                })
            })
            .expect("eu points");
        assert_eq!(
            eu_last.value,
            Some(number_data_point::Value::AsDouble(240.0))
        );
    }

    #[test]
    fn deep_chain_has_single_root_and_depth_fourteen() {
        let spans = t_deep();
        assert_eq!(spans.len(), 14);
        assert_eq!(roots(&spans), 1);
        for pair in spans.windows(2) {
            assert_eq!(pair[1].parent.as_deref(), Some(pair[0].id.as_slice()));
        }
    }

    #[test]
    fn wide_trace_exceeds_five_hundred_spans_in_one_trace() {
        let spans = t_wide();
        assert!(spans.len() > 500);
        assert_eq!(roots(&spans), 1);
        let trace = &spans[0].trace;
        assert!(spans.iter().all(|span| &span.trace == trace));
    }

    #[test]
    fn multiroot_trace_has_two_roots_in_one_trace() {
        let spans = t_multiroot();
        assert_eq!(roots(&spans), 2);
        assert_eq!(spans[0].trace, spans[1].trace);
    }

    #[test]
    fn orphan_child_references_a_parent_that_never_exports() {
        let spans = t_orphan();
        let exported: Vec<&[u8]> = spans.iter().map(|span| span.id.as_slice()).collect();
        let orphan = spans
            .iter()
            .find(|span| span.name == "orphan.detached_child")
            .expect("orphan span");
        assert!(!exported.contains(&orphan.parent.as_deref().expect("parent id")));
    }

    #[test]
    fn skewed_child_starts_before_its_parent() {
        let spans = t_skew();
        let parent = &spans[0];
        let child = &spans[1];
        assert_eq!(child.parent.as_deref(), Some(parent.id.as_slice()));
        assert!(child.start < parent.start, "negative skew required");
        assert!(
            parent.start - child.start > 50_000_000,
            "skew must exceed the 50 ms cross-service warning threshold"
        );
        assert_ne!(
            child.service, parent.service,
            "skew is cross-service by definition"
        );
        let request = traces_request(t_skew());
        assert_eq!(
            request.resource_spans.len(),
            2,
            "parent and child export under separate service resources"
        );
    }

    #[test]
    fn zero_duration_and_microsecond_twins_are_exact() {
        let spans = t_zero();
        let zero = spans
            .iter()
            .find(|span| span.name == "zero.instant")
            .unwrap();
        assert_eq!(zero.start, zero.end);
        let micro = spans
            .iter()
            .find(|span| span.name == "zero.microsecond_twin")
            .unwrap();
        assert_eq!(micro.end - micro.start, 1_000);
    }

    #[test]
    fn cross_trace_links_point_at_each_other() {
        let spans = t_links();
        assert_eq!(spans[0].links[0].trace_id, spans[1].trace);
        assert_eq!(spans[0].links[0].span_id, spans[1].id);
        assert_eq!(spans[1].links[0].trace_id, spans[0].trace);
        assert_eq!(spans[1].links[0].span_id, spans[0].id);
    }

    #[test]
    fn longnames_reach_kib_scale() {
        let spans = t_longnames();
        assert!(spans[0].name.len() >= 1024);
        let value = spans[0]
            .attrs
            .iter()
            .find(|attr| attr.key == "shape.long_value")
            .unwrap();
        let AnyValueEnum::StringValue(value) =
            value.value.as_ref().unwrap().value.as_ref().unwrap()
        else {
            panic!("string value")
        };
        assert!(value.len() >= 2048);
    }

    #[test]
    fn events_flood_carries_three_language_stacktraces() {
        let spans = t_events();
        assert!(spans[0].events.len() >= 50);
        let stacks = spans[0]
            .events
            .iter()
            .filter(|event| event.name == "exception")
            .count();
        assert_eq!(stacks, 3);
    }

    #[test]
    fn error_burst_repeats_one_type_and_adds_five_distinct() {
        let spans = e_burst();
        let recurring = spans
            .iter()
            .filter(|span| span.name == "burst.recurring_failure")
            .count();
        let distinct = spans
            .iter()
            .filter(|span| span.name == "burst.distinct_failure")
            .count();
        assert_eq!(recurring, 100);
        assert_eq!(distinct, 5);
    }

    #[test]
    fn log_burst_count_and_body_shapes_are_exact() {
        let burst = l_burst(5_000);
        assert_eq!(
            burst.resource_logs[0].scope_logs[0].log_records.len(),
            5_000
        );
        let bodies = l_bodies();
        let records = &bodies.resource_logs[0].scope_logs[0].log_records;
        assert!(records.iter().any(|record| matches!(
            record.body.as_ref().and_then(|body| body.value.as_ref()),
            Some(AnyValueEnum::StringValue(value)) if value.len() >= 32 * 1024
        )));
        let same_ts = records
            .iter()
            .filter(|record| {
                record.attributes.iter().any(|attr| {
                    attr.key == "shape.body"
                        && matches!(attr.value.as_ref().and_then(|v| v.value.as_ref()),
                            Some(AnyValueEnum::StringValue(value)) if value == "same-ts")
                })
            })
            .map(|record| record.time_unix_nano)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            same_ts.len(),
            1,
            "identical-timestamp run must share one ts"
        );
    }

    #[test]
    fn metric_shapes_cover_reset_gap_and_exemplar() {
        let anchor = m_shapes_anchor();
        let request = m_shapes(&anchor);
        let metrics = &request.resource_metrics[0].scope_metrics[0].metrics;
        assert_eq!(metrics.len(), 3);
        let Some(Data::Sum(sum)) = &metrics[0].data else {
            panic!("sum")
        };
        let values: Vec<f64> = sum
            .data_points
            .iter()
            .map(|point| match point.value {
                Some(number_data_point::Value::AsDouble(value)) => value,
                _ => panic!("double"),
            })
            .collect();
        assert!(values[2] < values[1], "counter must reset mid-window");
        let Some(Data::Histogram(histogram)) = &metrics[2].data else {
            panic!("histogram")
        };
        assert_eq!(histogram.data_points[0].exemplars.len(), 1);
        // The exemplar must deep-link to a trace that is actually exported.
        let exemplar = &histogram.data_points[0].exemplars[0];
        assert_eq!(exemplar.trace_id, anchor[0].trace);
        assert_eq!(exemplar.span_id, anchor[0].id);
        // The histogram rides the standard duration metric so the service
        // latency panel (the exemplar surface) renders it.
        assert_eq!(metrics[2].name, semconv::HTTP_SERVER_REQUEST_DURATION);
    }
}
