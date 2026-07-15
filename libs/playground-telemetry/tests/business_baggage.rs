use http::HeaderMap;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::global;
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use playground_telemetry::{
    extract_context, inject_context_headers, semconv, with_business_baggage,
};

#[test]
fn business_baggage_crosses_the_public_http_boundary() {
    global::set_text_map_propagator(opentelemetry::propagation::TextMapCompositePropagator::new(
        vec![
            Box::new(TraceContextPropagator::new()),
            Box::new(BaggagePropagator::new()),
        ],
    ));

    let mut headers = HeaderMap::new();
    let outbound = with_business_baggage(&opentelemetry::Context::new(), "tenant-a", "pro");
    inject_context_headers(&outbound, &mut headers);
    let downstream = extract_context(&headers);

    assert_eq!(
        downstream
            .baggage()
            .get(semconv::TENANT_ID)
            .map(ToString::to_string),
        Some("tenant-a".to_owned())
    );
    assert_eq!(
        downstream
            .baggage()
            .get(semconv::USER_TIER)
            .map(ToString::to_string),
        Some("pro".to_owned())
    );
}
