//! Shared OpenTelemetry + Sentry bootstrap for the Rust services.
//!
//! Dual pipeline (per the design doc): a single `tracing` subscriber feeds two
//! consumers — an `OpenTelemetryLayer` exporting OTLP spans to the collector, and
//! a `sentry-tracing` layer turning events into Sentry breadcrumbs/issues.
//! `tracing` is the only span source, so the two coexist without double
//! instrumentation. The OTLP endpoint is read from the standard
//! `OTEL_EXPORTER_OTLP_ENDPOINT` env (injected by `parallax run start` or the
//! lab), so pointing the whole app at Rotel needs no code change.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, global};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::resource::{SERVICE_NAME, SERVICE_VERSION};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialized telemetry. Hold the `_sentry` guard for the process lifetime and
/// call `shutdown()` before exit so buffered spans are flushed.
pub struct Telemetry {
    provider: SdkTracerProvider,
    _sentry: sentry::ClientInitGuard,
}

impl Telemetry {
    /// Flush + stop the exporter. Call before the process exits.
    pub fn shutdown(self) {
        let _ = self.provider.shutdown();
    }
}

/// Wire OTLP traces + a `tracing` subscriber + Sentry for `service`.
///
/// Reads `OTEL_EXPORTER_OTLP_ENDPOINT` (default per the OTLP SDK) and
/// `SENTRY_DSN` (Sentry disabled when unset). Honors `RUST_LOG`.
pub fn init(service: &'static str) -> anyhow::Result<Telemetry> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = Resource::builder()
        .with_attributes([
            KeyValue::new(SERVICE_NAME, service),
            KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;

    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();
    global::set_tracer_provider(provider.clone());

    let tracer = provider.tracer(service);

    // Sentry rides alongside; DSN from env, disabled gracefully when absent.
    let sentry = sentry::init(sentry::ClientOptions {
        dsn: std::env::var("SENTRY_DSN").ok().and_then(|d| d.parse().ok()),
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
        .with(sentry_tracing::layer())
        .init();

    Ok(Telemetry {
        provider,
        _sentry: sentry,
    })
}
