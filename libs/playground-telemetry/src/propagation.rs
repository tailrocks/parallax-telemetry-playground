use crate::semconv;
use http::HeaderMap;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::{Status, TraceContextExt};
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use std::collections::BTreeMap;
use tonic::metadata::{Ascii, KeyRef, MetadataKey, MetadataMap};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub fn extract_context(headers: &HeaderMap) -> Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)))
}

pub fn set_parent_from(headers: &HeaderMap) {
    set_parent_if_valid(&tracing::Span::current(), extract_context(headers));
}

pub fn set_parent_from_headers(span: &tracing::Span, headers: &HeaderMap) {
    set_parent_if_valid(span, extract_context(headers));
}

/// Copies the A10 business baggage into a server span for backend inspection.
pub fn stamp_business_baggage(span: &tracing::Span, context: &Context) {
    for key in [semconv::TENANT_ID, semconv::USER_TIER] {
        if let Some(value) = context.baggage().get(key) {
            span.set_attribute(key, value.to_string());
        }
    }
}

pub fn inject_context_headers(context: &Context, headers: &mut HeaderMap) {
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(context, &mut HeaderInjector(headers));
    });
}

pub fn inject_headers(headers: &mut HeaderMap) {
    inject_context_headers(&current_context(), headers);
}

/// Combine the current tracing span with W3C baggage attached to this task.
#[must_use]
pub fn current_context() -> Context {
    let baggage = Context::current()
        .baggage()
        .iter()
        .map(|(key, (value, _))| KeyValue::new(key.clone(), value.clone()))
        .collect::<Vec<_>>();
    tracing::Span::current().context().with_baggage(baggage)
}

/// Add the business context used by the A10 propagation scenario.
#[must_use]
pub fn with_business_baggage(context: &Context, tenant: &str, tier: &str) -> Context {
    context.with_baggage([
        KeyValue::new(semconv::TENANT_ID, tenant.to_owned()),
        KeyValue::new(semconv::USER_TIER, tier.to_owned()),
    ])
}

pub fn extract_context_from_env() -> Context {
    let carrier = EnvExtractor::from_env();
    global::get_text_map_propagator(|propagator| propagator.extract(&carrier))
}

pub fn set_parent_from_env(span: &tracing::Span) {
    set_parent_if_valid(span, extract_context_from_env());
}

pub fn context_env(context: &Context) -> Vec<(String, String)> {
    let mut carrier = EnvInjector::default();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(context, &mut carrier);
    });
    carrier.into_env()
}

pub fn current_context_env() -> Vec<(String, String)> {
    context_env(&tracing::Span::current().context())
}

pub async fn traced_get(url: &str) -> reqwest::Result<reqwest::Response> {
    let mut headers = HeaderMap::new();
    inject_headers(&mut headers);
    reqwest::Client::new()
        .get(url)
        .headers(headers)
        .send()
        .await
}

pub struct MetadataInjector<'a>(pub &'a mut MetadataMap);

impl Injector for MetadataInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(key) = MetadataKey::<Ascii>::from_bytes(key.as_bytes())
            && let Ok(value) = value.parse()
        {
            self.0.insert(key, value);
        }
    }
}

pub struct MetadataExtractor<'a>(pub &'a MetadataMap);

impl Extractor for MetadataExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0
            .keys()
            .filter_map(|key| match key {
                KeyRef::Ascii(key) => Some(key.as_str()),
                KeyRef::Binary(_) => None,
            })
            .collect()
    }

    fn get_all(&self, key: &str) -> Option<Vec<&str>> {
        let values = self
            .0
            .get_all(key)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>();
        (!values.is_empty()).then_some(values)
    }
}

#[derive(Default)]
struct EnvInjector(BTreeMap<&'static str, String>);

impl EnvInjector {
    fn into_env(self) -> Vec<(String, String)> {
        self.0
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }
}

impl Injector for EnvInjector {
    fn set(&mut self, key: &str, value: String) {
        if let Some(key) = env_key(key) {
            self.0.insert(key, value);
        }
    }
}

struct EnvExtractor {
    values: BTreeMap<&'static str, String>,
}

impl EnvExtractor {
    fn from_env() -> Self {
        let mut values = BTreeMap::new();
        for key in ["TRACEPARENT", "TRACESTATE", "BAGGAGE"] {
            if let Ok(value) = std::env::var(key)
                && !value.trim().is_empty()
            {
                values.insert(key, value);
            }
        }
        Self { values }
    }
}

impl Extractor for EnvExtractor {
    fn get(&self, key: &str) -> Option<&str> {
        env_key(key).and_then(|key| self.values.get(key).map(String::as_str))
    }

    fn keys(&self) -> Vec<&str> {
        self.values.keys().copied().collect()
    }
}

fn env_key(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().as_str() {
        "traceparent" => Some("TRACEPARENT"),
        "tracestate" => Some("TRACESTATE"),
        "baggage" => Some("BAGGAGE"),
        _ => None,
    }
}

pub fn set_parent_from_grpc(metadata: &MetadataMap) {
    set_parent_if_valid(&tracing::Span::current(), extract_grpc_context(metadata));
}

pub fn set_parent_from_grpc_metadata(span: &tracing::Span, metadata: &MetadataMap) {
    set_parent_if_valid(span, extract_grpc_context(metadata));
}

pub fn extract_grpc_context(metadata: &MetadataMap) -> Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&MetadataExtractor(metadata)))
}

pub fn inject_grpc_metadata(metadata: &mut MetadataMap) {
    let context = current_context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut MetadataInjector(metadata));
    });
}

pub fn mark_span_error(error_type: &'static str) {
    let span = tracing::Span::current();
    span.set_status(Status::error(error_type));
    span.set_attribute(semconv::ERROR_TYPE, error_type);
}

fn set_parent_if_valid(span: &tracing::Span, parent: Context) {
    if parent.span().span_context().is_valid() {
        let _ = span.set_parent(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
    use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn propagator_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("propagator lock")
    }

    #[test]
    fn http_headers_round_trip_trace_context() {
        let _guard = propagator_lock();
        global::set_text_map_propagator(TraceContextPropagator::new());
        let trace_id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id");
        let span_context = SpanContext::new(
            trace_id,
            SpanId::from_hex("00f067aa0ba902b7").expect("span id"),
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        let context = Context::new().with_remote_span_context(span_context);
        let mut headers = HeaderMap::new();

        inject_context_headers(&context, &mut headers);
        let extracted = extract_context(&headers);

        assert_eq!(extracted.span().span_context().trace_id(), trace_id);
    }

    #[test]
    fn env_carrier_injects_trace_context_names() {
        let _guard = propagator_lock();
        global::set_text_map_propagator(TraceContextPropagator::new());
        let trace_id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id");
        let span_context = SpanContext::new(
            trace_id,
            SpanId::from_hex("00f067aa0ba902b7").expect("span id"),
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        let context = Context::new().with_remote_span_context(span_context);
        let vars = context_env(&context);

        assert!(vars.iter().any(|(key, _)| key == "TRACEPARENT"));
        assert!(vars.iter().all(|(key, _)| key == &key.to_ascii_uppercase()));
    }

    #[test]
    fn business_baggage_round_trips_through_http_headers() -> Result<(), String> {
        let _guard = propagator_lock();
        global::set_text_map_propagator(
            opentelemetry::propagation::TextMapCompositePropagator::new(vec![
                Box::new(TraceContextPropagator::new()),
                Box::new(BaggagePropagator::new()),
            ]),
        );
        let context = with_business_baggage(&Context::new(), "tenant-a", "pro");
        let mut headers = HeaderMap::new();
        inject_context_headers(&context, &mut headers);
        let baggage = headers
            .get("baggage")
            .and_then(|value| value.to_str().ok())
            .ok_or("baggage header missing")?;
        let extracted = extract_context(&headers);
        let expected_header_members = ["tenant.id=tenant-a", "user.tier=pro"];
        let actual = (
            expected_header_members
                .iter()
                .all(|member| baggage.split(',').any(|actual| actual == *member)),
            extracted
                .baggage()
                .get(semconv::TENANT_ID)
                .map(ToString::to_string),
            extracted
                .baggage()
                .get(semconv::USER_TIER)
                .map(ToString::to_string),
        );
        if actual != (true, Some("tenant-a".to_string()), Some("pro".to_string())) {
            return Err(format!("baggage propagation mismatch: {actual:?}"));
        }
        Ok(())
    }
}
