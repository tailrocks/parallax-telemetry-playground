use http::HeaderMap;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::{Status, TraceContextExt};
use opentelemetry::{Context, global};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
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

pub fn inject_context_headers(context: &Context, headers: &mut HeaderMap) {
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(context, &mut HeaderInjector(headers));
    });
}

pub fn inject_headers(headers: &mut HeaderMap) {
    inject_context_headers(&tracing::Span::current().context(), headers);
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

pub fn set_parent_from_grpc(metadata: &MetadataMap) {
    let parent = global::get_text_map_propagator(|propagator| {
        propagator.extract(&MetadataExtractor(metadata))
    });
    set_parent_if_valid(&tracing::Span::current(), parent);
}

pub fn set_parent_from_grpc_metadata(span: &tracing::Span, metadata: &MetadataMap) {
    let parent = global::get_text_map_propagator(|propagator| {
        propagator.extract(&MetadataExtractor(metadata))
    });
    set_parent_if_valid(span, parent);
}

pub fn inject_grpc_metadata(metadata: &mut MetadataMap) {
    let context = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut MetadataInjector(metadata));
    });
}

pub fn mark_span_error(error_type: &'static str) {
    let span = tracing::Span::current();
    span.set_status(Status::error(error_type));
    span.set_attribute("error.type", error_type);
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
    use opentelemetry_sdk::propagation::TraceContextPropagator;

    #[test]
    fn http_headers_round_trip_trace_context() {
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
}
