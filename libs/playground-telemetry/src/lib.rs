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

mod feature_flags;
pub mod propagation;
pub mod semconv;

pub use feature_flags::feature_flag;
pub use propagation::{
    context_env, current_context, current_context_env, extract_context, extract_context_from_env,
    inject_context_headers, inject_grpc_metadata, inject_headers, mark_span_error, set_parent_from,
    set_parent_from_env, set_parent_from_grpc, set_parent_from_grpc_metadata,
    set_parent_from_headers, traced_get, with_business_baggage,
};

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::Response,
};
use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider as _, Severity};
use opentelemetry::propagation::TextMapCompositePropagator;
use opentelemetry::trace::{Span as _, SpanBuilder, TracerProvider as _};
use opentelemetry::{KeyValue, global};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub use semconv::TOKIO_RUNTIME_METRIC_NAMES;

static EVENT_LOGGER: OnceLock<SdkLogger> = OnceLock::new();

pub fn db_span(
    operation_name: &'static str,
    query_summary: &'static str,
    query_text: &'static str,
) -> tracing::Span {
    // Playground lab emits full query text on purpose. SQL uses bind params only;
    // never interpolate user input into `db.query.text`.
    tracing::info_span!(
        "postgres.query",
        otel.kind = semconv::SPAN_KIND_CLIENT,
        "db.system.name" = "postgresql",
        "db.namespace" = "playground",
        "db.operation.name" = operation_name,
        "db.query.summary" = query_summary,
        "db.query.text" = query_text,
        "server.address" = "postgres",
        "server.port" = 5432_i64,
    )
}

/// Axum middleware for stable HTTP semantic conventions and RED metrics.
///
/// Apply with `Router::layer(axum::middleware::from_fn(http_server_observability))`
/// after declaring routes, so `MatchedPath` resolves to the stable route rather
/// than a cardinality-unbounded request URI.
pub async fn http_server_observability(request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| path.clone(), |matched| matched.as_str().to_owned());
    let span = tracing::info_span!(
        "http.server.request",
        otel.kind = semconv::SPAN_KIND_SERVER,
        http.request.method = %method,
        http.route = %route,
        url.path = %path,
        http.response.status_code = tracing::field::Empty,
    );
    set_parent_from_headers(&span, request.headers());
    let started = Instant::now();
    let response = next.run(request).instrument(span.clone()).await;
    let status = response.status().as_u16();
    span.record("http.response.status_code", i64::from(status));
    if status >= 500 {
        mark_span_error("http.server.error");
    }
    global::meter("playground.http")
        .f64_histogram(semconv::HTTP_SERVER_REQUEST_DURATION)
        .with_unit("s")
        .build()
        .record(
            started.elapsed().as_secs_f64(),
            &[
                KeyValue::new(semconv::HTTP_REQUEST_METHOD, method),
                KeyValue::new(semconv::HTTP_ROUTE, route),
                KeyValue::new(semconv::HTTP_RESPONSE_STATUS_CODE, i64::from(status)),
            ],
        );
    response
}

/// Emit a typed OTel log event (EventName set) on the shared logs pipeline,
/// correlated to the current span context when one is active.
pub fn emit_event(name: &'static str, attrs: &[(&'static str, String)]) {
    let Some(logger) = EVENT_LOGGER.get() else {
        tracing::warn!(event.name = name, "typed event logger unavailable");
        return;
    };
    if !logger.event_enabled(Severity::Info, "playground.events", Some(name)) {
        return;
    }
    let mut record = logger.create_log_record();
    populate_event_record(&mut record, name, attrs);
    logger.emit(record);
}

/// Emit a child span whose timestamps are deliberately behind the current span.
pub fn emit_backdated_span(name: &'static str, offset: Duration, duration: Duration) {
    let now = SystemTime::now();
    let start = now.checked_sub(offset).unwrap_or(SystemTime::UNIX_EPOCH);
    let end = start.checked_add(duration).unwrap_or(start);
    let parent = tracing::Span::current().context();
    let tracer = global::tracer("playground.backdated");
    let mut span = SpanBuilder::from_name(name)
        .with_start_time(start)
        .start_with_context(&tracer, &parent);
    span.end_with_timestamp(end);
}

fn populate_event_record<R>(record: &mut R, name: &'static str, attrs: &[(&'static str, String)])
where
    R: LogRecord,
{
    record.set_event_name(name);
    record.set_target("playground.events");
    record.set_severity_number(Severity::Info);
    record.set_severity_text("INFO");
    record.set_body(AnyValue::from(name));
    record.add_attribute(semconv::EVENT_NAME, name);
    for (key, value) in attrs {
        record.add_attribute(*key, value.clone());
    }
}

/// Initialized telemetry. Hold the `_sentry` guard for the process lifetime and
/// call `shutdown()` before exit so buffered spans/logs/metrics are flushed.
pub struct Telemetry {
    tracer_provider: SdkTracerProvider,
    logger_provider: SdkLoggerProvider,
    meter_provider: SdkMeterProvider,
    runtime_metrics: Option<tokio::task::JoinHandle<()>>,
    _sentry: sentry::ClientInitGuard,
}

impl Telemetry {
    /// Flush + stop every exporter. Call before the process exits.
    pub fn shutdown(self) {
        if let Some(task) = self.runtime_metrics {
            task.abort();
        }
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
    let release = release();

    // --- Traces ---
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;
    let sample_ratio = sample_ratio_from(std::env::var("PLAYGROUND_SAMPLE_RATIO").ok().as_deref());
    let mut tracer_builder = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter);
    if let SampleRatioSetting::Ratio(ratio) = sample_ratio {
        tracer_builder = tracer_builder.with_sampler(Sampler::ParentBased(Box::new(
            Sampler::TraceIdRatioBased(ratio),
        )));
    }
    let tracer_provider = tracer_builder.build();
    global::set_tracer_provider(tracer_provider.clone());
    let tracer = tracer_provider.tracer(service);

    // --- Metrics --- (Counter/Histogram emitted from `tracing` fields by MetricsLayer)
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .build()?;
    let metric_reader = PeriodicReader::builder(metric_exporter)
        .with_interval(metric_export_interval())
        .build();
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(metric_reader)
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
    let _ = EVENT_LOGGER.set(logger_provider.logger("playground.events"));
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
        release: Some(release.into()),
        environment: Some(environment_from(std::env::var("PARALLAX_ENV").ok()).into()),
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
    match sample_ratio {
        SampleRatioSetting::Ratio(ratio) => {
            tracing::info!(sample_ratio = ratio, "PLAYGROUND_SAMPLE_RATIO active");
        }
        SampleRatioSetting::Invalid => {
            tracing::warn!(
                "invalid PLAYGROUND_SAMPLE_RATIO; expected 0.0..=1.0, sampling default unchanged"
            );
        }
        SampleRatioSetting::Unset => {}
    }

    let runtime_metrics = spawn_runtime_metrics();

    Ok(Telemetry {
        tracer_provider,
        logger_provider,
        meter_provider,
        runtime_metrics,
        _sentry: sentry,
    })
}

pub fn spawn_runtime_metrics() -> Option<tokio::task::JoinHandle<()>> {
    if !runtime_metrics_enabled() {
        return None;
    }
    let handle = match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle,
        Err(err) => {
            tracing::warn!(error = %err, "tokio runtime metrics disabled outside a Tokio runtime");
            return None;
        }
    };
    let meter = global::meter("playground.runtime");
    let workers_count = meter
        .u64_gauge(TOKIO_RUNTIME_METRIC_NAMES[0])
        .with_description("Tokio worker threads configured for the runtime")
        .build();
    let alive_tasks = meter
        .u64_gauge(TOKIO_RUNTIME_METRIC_NAMES[1])
        .with_description("Tokio tasks alive in the runtime")
        .build();
    let global_queue_depth = meter
        .u64_gauge(TOKIO_RUNTIME_METRIC_NAMES[2])
        .with_description("Tokio global queue depth")
        .build();
    let total_park_count = meter
        .u64_gauge(TOKIO_RUNTIME_METRIC_NAMES[4])
        .with_description("Tokio worker park count for the sample interval")
        .build();
    let total_busy_duration_ms = meter
        .u64_gauge(TOKIO_RUNTIME_METRIC_NAMES[5])
        .with_description("Tokio worker busy duration for the sample interval")
        .build();

    Some(tokio::spawn(async move {
        let monitor = tokio_metrics::RuntimeMonitor::new(&handle);
        let mut intervals = monitor.intervals();
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let Some(interval) = intervals.next() else {
                tracing::warn!("tokio runtime metrics iterator ended");
                return;
            };
            workers_count.record(interval.workers_count as u64, &[]);
            alive_tasks.record(interval.live_tasks_count as u64, &[]);
            global_queue_depth.record(interval.global_queue_depth as u64, &[]);
            total_park_count.record(interval.total_park_count, &[]);
            total_busy_duration_ms.record(interval.total_busy_duration.as_millis() as u64, &[]);
            // Stable build note: tokio-metrics 0.5 exposes these shared runtime
            // fields without `tokio_unstable`. Blocking queue/thread fields,
            // budget-forced yield counts, poll histograms, worker-local queue
            // distributions, and schedule-source counters are unstable here.
            // Checkout publishes `tokio.runtime.blocking_pool_depth` from its
            // bounded A22 blocking-flood knob so the demo keeps a stable signal.
        }
    }))
}

fn runtime_metrics_enabled() -> bool {
    std::env::var("PLAYGROUND_TOKIO_METRICS")
        .map(|value| {
            let value = value.trim();
            !(value == "0"
                || value.eq_ignore_ascii_case("false")
                || value.eq_ignore_ascii_case("off")
                || value.eq_ignore_ascii_case("no"))
        })
        .unwrap_or(true)
}

fn metric_export_interval() -> Duration {
    std::env::var("PLAYGROUND_METRIC_EXPORT_INTERVAL_MS")
        .or_else(|_| std::env::var("OTEL_METRIC_EXPORT_INTERVAL"))
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_secs(5))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SampleRatioSetting {
    Unset,
    Ratio(f64),
    Invalid,
}

fn sample_ratio_from(value: Option<&str>) -> SampleRatioSetting {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return SampleRatioSetting::Unset;
    };
    match value.parse::<f64>() {
        Ok(ratio) if ratio.is_finite() && (0.0..=1.0).contains(&ratio) => {
            SampleRatioSetting::Ratio(ratio)
        }
        _ => SampleRatioSetting::Invalid,
    }
}

fn resource_attributes(service: &'static str) -> Vec<KeyValue> {
    let mut attributes = vec![
        KeyValue::new(semconv::SERVICE_NAME, service),
        KeyValue::new(semconv::SERVICE_VERSION, release()),
        KeyValue::new(semconv::SERVICE_NAMESPACE, semconv::PLAYGROUND_NAMESPACE),
        KeyValue::new(semconv::SERVICE_INSTANCE_ID, service_instance_id(service)),
        KeyValue::new(
            semconv::DEPLOYMENT_ENVIRONMENT_NAME,
            environment_from(std::env::var("PARALLAX_ENV").ok()),
        ),
    ];
    let otel_resource_attributes = std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok();
    if let Ok(run_id) = std::env::var("PARALLAX_RUN_ID")
        && !run_id.trim().is_empty()
        && !resource_attr_list_contains(
            otel_resource_attributes.as_deref(),
            semconv::PARALLAX_RUN_ID,
        )
    {
        attributes.push(KeyValue::new(semconv::PARALLAX_RUN_ID, run_id));
    }
    if let Some(git_sha) = non_empty_env("GIT_SHA") {
        attributes.push(KeyValue::new("vcs.ref.head.revision", git_sha));
    }
    attributes
}

fn service_instance_id(service: &str) -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| format!("{service}-{}", std::process::id()))
}

fn release() -> String {
    release_from(std::env::var("RELEASE").ok())
}

fn release_from(value: Option<String>) -> String {
    value
        .and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn environment_from(value: Option<String>) -> String {
    value
        .and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
        .unwrap_or_else(|| semconv::DEFAULT_ENVIRONMENT.to_string())
}

fn resource_attr_list_contains(value: Option<&str>, attr: &str) -> bool {
    value.is_some_and(|value| {
        value.split(',').any(|pair| {
            let Some((key, _value)) = pair.split_once('=') else {
                return false;
            };
            key.trim() == attr
        })
    })
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_sdk::logs::{LogBatch, LogExporter, SdkLogRecord, SdkLoggerProvider};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Default)]
    struct CaptureLogExporter {
        records: Arc<Mutex<Vec<SdkLogRecord>>>,
    }

    impl LogExporter for CaptureLogExporter {
        fn export(
            &self,
            batch: LogBatch<'_>,
        ) -> impl std::future::Future<Output = opentelemetry_sdk::error::OTelSdkResult> + Send
        {
            let records = self.records.clone();
            let owned = batch
                .iter()
                .map(|(record, _scope)| (*record).clone())
                .collect::<Vec<_>>();
            async move {
                records.lock().unwrap().extend(owned);
                Ok(())
            }
        }
    }

    #[test]
    fn release_uses_env_value_when_present() {
        assert_eq!(release_from(Some("v2".to_string())), "v2");
    }

    #[test]
    fn release_falls_back_to_crate_version() {
        assert_eq!(release_from(None), env!("CARGO_PKG_VERSION"));
        assert_eq!(
            release_from(Some("  ".to_string())),
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn environment_defaults_to_playground() {
        assert_eq!(environment_from(None), semconv::DEFAULT_ENVIRONMENT);
        assert_eq!(
            environment_from(Some("  ".to_string())),
            semconv::DEFAULT_ENVIRONMENT
        );
        assert_eq!(environment_from(Some("prod".to_string())), "prod");
    }

    #[test]
    fn resource_attr_list_detects_existing_run_id() {
        assert!(resource_attr_list_contains(
            Some("service.name=checkout, parallax.run.id=run-a"),
            semconv::PARALLAX_RUN_ID
        ));
        assert!(!resource_attr_list_contains(
            Some("service.name=checkout, parallax.session.id=s1"),
            semconv::PARALLAX_RUN_ID
        ));
        assert!(!resource_attr_list_contains(None, semconv::PARALLAX_RUN_ID));
    }

    #[test]
    fn shared_wire_names_are_frozen() {
        assert_eq!(semconv::PARALLAX_RUN_ID, "parallax.run.id");
        assert_eq!(semconv::SERVICE_NAME, "service.name");
        assert_eq!(semconv::SERVICE_VERSION, "service.version");
        assert_eq!(
            semconv::DEPLOYMENT_ENVIRONMENT_NAME,
            "deployment.environment.name"
        );
        assert_eq!(semconv::EVENT_NAME, "event.name");
        assert_eq!(semconv::APP_SCREEN_NAME, "app.screen.name");
        assert_eq!(semconv::OTEL_KIND, "otel.kind");
    }

    #[test]
    fn tokio_runtime_metric_names_match_runtime_lane_contract() {
        assert_eq!(
            TOKIO_RUNTIME_METRIC_NAMES,
            &[
                "tokio.runtime.workers_count",
                "tokio.runtime.alive_tasks",
                "tokio.runtime.global_queue_depth",
                "tokio.runtime.blocking_pool_depth",
                "tokio.runtime.total_park_count",
                "tokio.runtime.total_busy_duration_ms",
            ]
        );
    }

    #[test]
    fn backdated_span_helper_compiles_against_otel_api() {
        emit_backdated_span(
            "test-backdated",
            Duration::from_secs(60),
            Duration::from_millis(5),
        );
    }

    #[test]
    fn sample_ratio_parser_keeps_default_when_unset() {
        assert!(matches!(sample_ratio_from(None), SampleRatioSetting::Unset));
        assert!(matches!(
            sample_ratio_from(Some("  ")),
            SampleRatioSetting::Unset
        ));
    }

    #[test]
    fn sample_ratio_parser_accepts_bounded_ratio() {
        match sample_ratio_from(Some("0.1")) {
            SampleRatioSetting::Ratio(ratio) => {
                assert!((ratio - 0.1).abs() < f64::EPSILON);
            }
            other => panic!("expected ratio, got {other:?}"),
        }
    }

    #[test]
    fn sample_ratio_parser_rejects_junk_and_out_of_range() {
        assert!(matches!(
            sample_ratio_from(Some("junk")),
            SampleRatioSetting::Invalid
        ));
        assert!(matches!(
            sample_ratio_from(Some("-0.1")),
            SampleRatioSetting::Invalid
        ));
        assert!(matches!(
            sample_ratio_from(Some("1.1")),
            SampleRatioSetting::Invalid
        ));
    }

    #[test]
    fn typed_event_record_sets_name_and_attrs() {
        let exporter = CaptureLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let logger = provider.logger("test.events");
        let mut record = logger.create_log_record();
        populate_event_record(
            &mut record,
            "checkout.completed",
            &[("sku", "WIDGET-1".to_string())],
        );
        logger.emit(record);

        let logs = exporter.records.lock().unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].event_name(), Some("checkout.completed"));
        let attrs = logs[0]
            .attributes_iter()
            .map(|(key, value)| (key.to_string(), format!("{value:?}")))
            .collect::<Vec<_>>();
        assert!(
            attrs
                .iter()
                .any(|(key, value)| key == "sku" && value.contains("WIDGET-1"))
        );
        assert!(
            attrs
                .iter()
                .any(|(key, value)| key == semconv::EVENT_NAME
                    && value.contains("checkout.completed"))
        );
    }
}
