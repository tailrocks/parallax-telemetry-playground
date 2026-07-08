//! Shared OpenTelemetry + Sentry bootstrap for the Rust services.
//!
//! Dual pipeline (per the design doc §4/§8): a single `tracing` subscriber feeds
//! parallel consumers so `tracing` is the only span source and nothing is
//! double-instrumented —
//!   * `OpenTelemetryLayer`  — OTLP **traces** → collector,
//!   * `MetricsLayer`        — OTLP **metrics** (counters/histograms from
//!     `tracing` fields) → collector,
//!   * `OpenTelemetryTracingBridge` — `tracing` events → OTLP **logs**,
//!     auto-stamped with the active trace/span id,
//!   * `sentry-tracing`      — events → Sentry breadcrumbs/issues.
//!
//! All three OTLP signals share one `Resource` and target the standard
//! `OTEL_EXPORTER_OTLP_ENDPOINT` (injected by `parallax run start` or the lab),
//! so pointing the whole app at Rotel needs no code change.
//!
//! (Metric **exemplars** are intentionally absent — the Rust SDK doesn't
//! implement them yet, issue #3369; the JVM tier is the playground's exemplar
//! source.)

pub mod propagation;

pub use propagation::{
    inject_context_headers, inject_grpc_metadata, inject_headers, mark_span_error, set_parent_from,
    set_parent_from_grpc, set_parent_from_grpc_metadata, set_parent_from_headers, traced_get,
};

use opentelemetry::propagation::TextMapCompositePropagator;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, global};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::resource::{SERVICE_NAME, SERVICE_VERSION};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialized telemetry. Hold the `_sentry` guard for the process lifetime and
/// call `shutdown()` before exit so buffered spans/logs/metrics are flushed.
pub struct Telemetry {
    tracer_provider: SdkTracerProvider,
    logger_provider: SdkLoggerProvider,
    meter_provider: SdkMeterProvider,
    _sentry: sentry::ClientInitGuard,
}

impl Telemetry {
    /// Flush + stop every exporter. Call before the process exits.
    pub fn shutdown(self) {
        let _ = self.tracer_provider.shutdown();
        let _ = self.logger_provider.shutdown();
        let _ = self.meter_provider.shutdown();
    }
}

pub async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %err, "failed to install Ctrl-C shutdown signal");
    }
}

/// Wire OTLP traces + metrics + logs + a `tracing` subscriber + Sentry for
/// `service`.
///
/// Reads `OTEL_EXPORTER_OTLP_ENDPOINT` (default per the OTLP SDK) and
/// `SENTRY_DSN` (Sentry disabled when unset). Honors `RUST_LOG`.
pub fn init(service: &'static str) -> anyhow::Result<Telemetry> {
    global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
        Box::new(TraceContextPropagator::new()),
        Box::new(BaggagePropagator::new()),
    ]));

    let resource = Resource::builder()
        .with_attributes(resource_attributes(service))
        .build();

    // --- Traces ---
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();
    global::set_tracer_provider(tracer_provider.clone());
    let tracer = tracer_provider.tracer(service);

    // --- Metrics --- (Counter/Histogram emitted from `tracing` fields by MetricsLayer)
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_periodic_exporter(metric_exporter)
        .build();
    global::set_meter_provider(meter_provider.clone());

    // --- Logs --- (`tracing` events → OTLP LogRecords, trace-correlated)
    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .build()?;
    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(log_exporter)
        .build();
    // Drop the transport crates' own logs from the OTLP log layer, else exporting
    // a log emits a log → feedback loop (doc §8).
    let log_layer =
        OpenTelemetryTracingBridge::new(&logger_provider).with_filter(filter_fn(|meta| {
            let t = meta.target();
            !(t.starts_with("hyper")
                || t.starts_with("tonic")
                || t.starts_with("h2")
                || t.starts_with("reqwest")
                || t.starts_with("opentelemetry")
                || t.starts_with("tower"))
        }));

    // Sentry rides alongside; DSN from env, disabled gracefully when absent.
    let sentry = sentry::init(sentry::ClientOptions {
        dsn: std::env::var("SENTRY_DSN")
            .ok()
            .and_then(|d| d.parse().ok()),
        release: Some(env!("CARGO_PKG_VERSION").into()),
        environment: Some(
            std::env::var("PARALLAX_ENV")
                .unwrap_or_else(|_| "lab".into())
                .into(),
        ),
        traces_sample_rate: 1.0,
        attach_stacktrace: true,
        send_default_pii: false,
        ..Default::default()
    });

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .with(tracing_opentelemetry::MetricsLayer::new(
            meter_provider.clone(),
        ))
        .with(log_layer)
        .with(sentry_tracing::layer())
        .init();

    Ok(Telemetry {
        tracer_provider,
        logger_provider,
        meter_provider,
        _sentry: sentry,
    })
}

fn resource_attributes(service: &'static str) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(SERVICE_NAME, service),
        KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
        KeyValue::new("service.namespace", "playground"),
        KeyValue::new("service.instance.id", service_instance_id(service)),
        KeyValue::new(
            "deployment.environment.name",
            std::env::var("PARALLAX_ENV").unwrap_or_else(|_| "lab".into()),
        ),
    ];
    if let Ok(run_id) = std::env::var("PARALLAX_RUN_ID")
        && !otel_resource_attrs_has("parallax.run.id")
    {
        attributes.push(KeyValue::new("parallax.run.id", run_id));
    }
    attributes
}

fn service_instance_id(service: &str) -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| format!("{service}-{}", std::process::id()))
}

fn otel_resource_attrs_has(key: &str) -> bool {
    std::env::var("OTEL_RESOURCE_ATTRIBUTES").is_ok_and(|attrs| {
        attrs
            .split(',')
            .filter_map(|item| item.split_once('='))
            .any(|(name, _)| name.trim() == key)
    })
}
