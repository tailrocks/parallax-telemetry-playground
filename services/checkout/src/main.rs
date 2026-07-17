//! Checkout HTTP service (axum) — the orchestrator / trace spine. One
//! `GET /checkout` fans out to pricing (gRPC), inventory (HTTP) and
//! recommendation (HTTP), producing a multi-service distributed trace
//! (HTTP SERVER → gRPC CLIENT + HTTP CLIENT spans → each downstream SERVER span).
//!
//! Deliberate-chaos + canary knobs are per-request query params (so scenarios are
//! deterministic) and also honor ambient flags:
//!   ?fail=1      payment failure → 502 + error issue        (B1)
//!   ?slow=<ms>   injected latency                            (B11)
//!   ?canary=1    plant a redaction canary corpus in span/log (A18)
//!   ?block_ms=<n>&block_n=<m> flood spawn_blocking sleeps     (A22)
//!   ?spike=<screen> emit dominant structured WARN logs         (A9)
//!   ?rogue_log=1  emit one detached log without trace context  (B23)

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::IntoResponse;
use axum::{Json, Router, extract::Query, routing::get};
use opentelemetry::trace::TraceContextExt;
use playground_proto::pricing::v1::QuoteRequest;
use playground_proto::pricing::v1::pricing_client::PricingClient;
use playground_telemetry::semconv;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{future::Future, pin::Pin};
use tonic::Code;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

static BLOCKING_POOL_DEPTH: AtomicU64 = AtomicU64::new(0);
const MAX_BLOCK_MS: u64 = 30_000;
const MAX_BLOCK_N: u32 = 1_024;
const MAX_BURST_FAN: u32 = 50;
const MAX_BURST_DEPTH: u32 = 10;
const MAX_BURST_SPANS: u64 = 2_000;

#[derive(Debug)]
struct PaymentError;

impl std::fmt::Display for PaymentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PaymentError: payment failed")
    }
}

impl std::error::Error for PaymentError {}

/// Query flags arrive as `1`/`true`/`yes`/`on`; serde's bool wants `true`/`false`.
fn de_flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let s = String::deserialize(d)?;
    Ok(matches!(s.as_str(), "1" | "true" | "yes" | "on"))
}

#[derive(Deserialize)]
struct CheckoutParams {
    #[serde(default = "default_sku")]
    sku: String,
    #[serde(default = "default_qty")]
    quantity: u32,
    #[serde(default, deserialize_with = "de_flag")]
    fail: bool,
    #[serde(default)]
    slow: u64,
    #[serde(default, deserialize_with = "de_flag")]
    canary: bool,
    /// B9: extra sequential inventory calls (N+1 hotspot).
    #[serde(default)]
    n1: u32,
    /// A19: synthetic trace shape width.
    #[serde(default)]
    fan: u32,
    /// A19: synthetic trace shape depth.
    #[serde(default)]
    depth: u32,
    /// A20: green structural compare variant.
    variant: Option<String>,
    /// B3: retry the pricing call up to N times with a per-attempt deadline.
    #[serde(default)]
    retry: u32,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    /// B3b/A7b: ask pricing to delay work so grpc-timeout and streams are visible.
    #[serde(default)]
    delay_ms: u32,
    /// A7b: fail the pricing stream at this message index.
    #[serde(default)]
    fail_at: u32,
    /// A7b: cancel the pricing stream client-side after this many ms.
    #[serde(default)]
    cancel_ms: u64,
    /// B5: busy-loop for this many ms (high-CPU hot path).
    #[serde(default)]
    cpu_ms: u64,
    /// B10: hold a shared lock during the request (connection-pool/mutex
    /// contention under concurrency).
    #[serde(default, deserialize_with = "de_flag")]
    lock: bool,
    /// A10: business context carried as baggage (tenant + tier).
    tenant: Option<String>,
    #[serde(default = "default_tier")]
    tier: String,
    /// B4: on a pricing failure, degrade to a partial 200 instead of 502.
    #[serde(default, deserialize_with = "de_flag")]
    degrade: bool,
    /// B18: emit a genuinely backdated child span.
    #[serde(default, deserialize_with = "de_flag")]
    skew: bool,
    /// A22: flood Tokio's blocking pool with bounded sleeping tasks.
    #[serde(default)]
    block_ms: u64,
    #[serde(default)]
    block_n: u32,
    /// A9: emit a burst of logs with one dominant field value.
    spike: Option<String>,
    /// B23: emit a detached log outside span context.
    #[serde(default, deserialize_with = "de_flag")]
    rogue_log: bool,
}

fn default_tier() -> String {
    "free".into()
}

/// B10: a process-wide lock to serialize requests on demand (contention demo).
fn contention_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn default_timeout() -> u64 {
    1000
}

fn default_sku() -> String {
    "WIDGET-1".into()
}
fn default_qty() -> u32 {
    1
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

async fn checkout(headers: HeaderMap, Query(p): Query<CheckoutParams>) -> impl IntoResponse {
    let span = tracing::info_span!("checkout", otel.kind = semconv::SPAN_KIND_SERVER);
    let parent = playground_telemetry::extract_context(&headers);
    let parent = p.tenant.as_deref().map_or(parent.clone(), |tenant| {
        playground_telemetry::with_business_baggage(&parent, tenant, &p.tier)
    });
    if parent.span().span_context().is_valid() {
        let _ = span.set_parent(parent);
    }
    checkout_inner(p).instrument(span).await
}

async fn checkout_inner(p: CheckoutParams) -> impl IntoResponse {
    // An explicit scenario parameter must stay deterministic even when flagd is
    // unavailable: it is the direct B1 contract and should not wait on three
    // unrelated remote flag evaluations before returning its deliberate error.
    let (payment_failure_flag, slow_query_flag, canary_failure_flag) = if p.fail {
        (false, false, false)
    } else {
        (
            playground_telemetry::feature_flag("paymentFailure", "PAYMENT_FAILURE").await,
            playground_telemetry::feature_flag("slowQuery", "SLOW_QUERY").await,
            playground_telemetry::feature_flag("canaryFailure", "CANARY_FAILURE").await,
        )
    };
    let slow_ms = if p.slow > 0 {
        p.slow
    } else if slow_query_flag {
        250
    } else {
        0
    };
    if slow_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(slow_ms)).await;
    }
    if let Some(screen) = p
        .spike
        .as_deref()
        .map(str::trim)
        .filter(|screen| !screen.is_empty())
    {
        emit_field_spike(screen);
    }
    if p.rogue_log {
        emit_rogue_log();
    }
    // A10: business context is attached as W3C baggage and injected by every
    // downstream HTTP/gRPC helper for this request.
    if let Some(tenant) = &p.tenant {
        tracing::Span::current().set_attribute(semconv::TENANT_ID, tenant.clone());
        tracing::Span::current().set_attribute(semconv::USER_TIER, p.tier.clone());
        tracing::info!(tenant.id = %tenant, user.tier = %p.tier, "baggage business context");
    }
    // B10: contention — serialize on a shared lock while held.
    let _guard = if p.lock {
        tracing::info!("acquiring shared lock (contention)");
        Some(contention_lock().lock().await)
    } else {
        None
    };
    // B5: high-CPU hot path — busy-loop for cpu_ms.
    if p.cpu_ms > 0 {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(p.cpu_ms);
        let mut x: u64 = 0;
        while std::time::Instant::now() < until {
            x = x.wrapping_add(1);
        }
        tracing::warn!(cpu_ms = p.cpu_ms, iterations = x, "high-CPU hot path");
    }
    if p.block_ms > 0 && p.block_n > 0 {
        flood_blocking_pool(p.block_ms, p.block_n).await;
    }
    if p.canary || env_flag("CANARY") || canary_failure_flag {
        // A18: plant a redaction canary corpus so backends can be compared on
        // raw-vs-scrubbed. These are FAKE secrets for redaction testing only.
        tracing::warn!(
            canary.email = "alice@example.com",
            canary.token = "sk-live-CANARY1234567890",
            canary.card = "4111111111111111",
            canary.jwt = "eyJhbGciOiJIUzI1NiJ9.CANARY.sig",
            "canary payload planted for redaction comparison"
        );
    }
    if p.skew {
        // B18: backdate a child span under the current checkout span. The log
        // below is only a witness; the skew is in the emitted span timestamp.
        let skewed = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let skewed = skewed
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        playground_telemetry::emit_backdated_span(
            "skewed-op",
            std::time::Duration::from_secs(3600),
            std::time::Duration::from_millis(20),
        );
        tracing::warn!(skewed_unix_s = skewed, "clock-skew event (1h in the past)");
    }
    // B12: release-attributed regression — RELEASE=v2 fails (vs v1 clean).
    let release_regressed = std::env::var("RELEASE").as_deref() == Ok("v2");
    if p.fail || payment_failure_flag || release_regressed {
        // B1/B12: deliberate failure → error issue + ERROR span status.
        let error = PaymentError;
        playground_telemetry::mark_span_error("PaymentError");
        playground_telemetry::emit_event(
            "checkout.failed",
            &[
                ("sku", p.sku.clone()),
                (semconv::ERROR_TYPE, "PaymentError".to_string()),
                ("error.message", error.to_string()),
            ],
        );
        tracing::error!(error = %error, sku = %p.sku, payment_failure_flag, release_regressed, "payment failure (chaos)");
        // B4: cascading failure → degrade to a partial 200 when asked, else 502.
        if p.degrade {
            tracing::warn!("degraded: returning partial result without pricing");
            let inventory = reserve(&p.sku, p.quantity)
                .await
                .unwrap_or(json!({"error": "unavailable"}));
            return (
                StatusCode::OK,
                Json(json!({ "sku": p.sku, "degraded": true, "inventory": inventory })),
            );
        }
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": "payment failed", "sku": p.sku })),
        );
    }

    let variant = compare_variant(p.variant.as_deref());
    tracing::Span::current().set_attribute("compare.variant", variant);
    tracing::info!(compare.variant = variant, "checkout compare variant");

    let n1_count = if variant == "v2" { p.n1.max(8) } else { p.n1 };
    // B9/A20: N+1 — fire extra sequential inventory calls (a classic hotspot).
    for i in 0..n1_count {
        let _ = reserve(&p.sku, 1).await;
        tracing::debug!(i, "n+1 inventory call");
    }

    let (fan, depth) = clamp_shape(p.fan, p.depth);
    if fan > 0 && depth > 0 {
        tracing::info!(
            requested_fan = p.fan,
            requested_depth = p.depth,
            fan,
            depth,
            estimated_spans = estimated_burst_spans(fan, depth),
            "synthetic burst trace requested"
        );
        burst(1, fan, depth).await;
    }

    let pricing = quote_with_retry(&p.sku, p.quantity, p.retry, p.timeout_ms, p.delay_ms).await;
    let inventory = reserve(&p.sku, p.quantity).await;
    let recommendation = if variant == "v2" {
        Ok(json!({"skipped": "compare.variant=v2"}))
    } else {
        recommend(&p.sku).await
    };

    match pricing {
        Ok((total, currency)) => {
            playground_telemetry::emit_event(
                "checkout.completed",
                &[
                    ("sku", p.sku.clone()),
                    ("quantity", p.quantity.to_string()),
                    ("order.total", total.to_string()),
                ],
            );
            tracing::info!(sku = %p.sku, total, "checkout ok");
            (
                StatusCode::OK,
                Json(json!({
                    "sku": p.sku,
                    "quantity": p.quantity,
                    "total_minor": total,
                    "currency": currency,
                    "inventory": inventory.unwrap_or(json!({"error": "unavailable"})),
                    "recommendation": recommendation.unwrap_or(json!({"error": "unavailable"})),
                })),
            )
        }
        Err(err) => {
            playground_telemetry::mark_span_error("pricing_unavailable");
            playground_telemetry::emit_event(
                "checkout.failed",
                &[
                    ("sku", p.sku.clone()),
                    (semconv::ERROR_TYPE, "pricing_unavailable".to_string()),
                ],
            );
            tracing::error!(error = %err, "pricing call failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": err.to_string() })),
            )
        }
    }
}

fn compare_variant(variant: Option<&str>) -> &'static str {
    match variant.map(str::trim) {
        Some("v2") => "v2",
        _ => "v1",
    }
}

fn estimated_burst_spans(fan: u32, depth: u32) -> u64 {
    let mut total = 0_u64;
    let mut level = 1_u64;
    for _ in 0..depth {
        level = level.saturating_mul(u64::from(fan));
        total = total.saturating_add(level);
        if total > MAX_BURST_SPANS {
            return total;
        }
    }
    total
}

fn clamp_shape(fan: u32, depth: u32) -> (u32, u32) {
    let mut fan = fan.min(MAX_BURST_FAN);
    let depth = depth.min(MAX_BURST_DEPTH);
    if fan == 0 || depth == 0 {
        return (0, 0);
    }
    while fan > 1 && estimated_burst_spans(fan, depth) > MAX_BURST_SPANS {
        fan -= 1;
    }
    (fan, depth)
}

fn burst_span(level: u32) -> tracing::Span {
    match level {
        1 => tracing::info_span!("burst.l1", otel.kind = semconv::SPAN_KIND_INTERNAL),
        2 => tracing::info_span!("burst.l2", otel.kind = semconv::SPAN_KIND_INTERNAL),
        3 => tracing::info_span!("burst.l3", otel.kind = semconv::SPAN_KIND_INTERNAL),
        4 => tracing::info_span!("burst.l4", otel.kind = semconv::SPAN_KIND_INTERNAL),
        5 => tracing::info_span!("burst.l5", otel.kind = semconv::SPAN_KIND_INTERNAL),
        6 => tracing::info_span!("burst.l6", otel.kind = semconv::SPAN_KIND_INTERNAL),
        7 => tracing::info_span!("burst.l7", otel.kind = semconv::SPAN_KIND_INTERNAL),
        8 => tracing::info_span!("burst.l8", otel.kind = semconv::SPAN_KIND_INTERNAL),
        9 => tracing::info_span!("burst.l9", otel.kind = semconv::SPAN_KIND_INTERNAL),
        _ => tracing::info_span!("burst.l10", otel.kind = semconv::SPAN_KIND_INTERNAL),
    }
}

fn burst(level: u32, width: u32, depth: u32) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async move {
        if level > depth {
            return;
        }
        let mut handles = Vec::with_capacity(width as usize);
        for index in 0..width {
            let span = burst_span(level);
            let delay = std::time::Duration::from_millis(u64::from((level + index) % 5 + 1));
            handles.push(tokio::spawn(
                async move {
                    tokio::time::sleep(delay).await;
                    burst(level + 1, width, depth).await;
                }
                .instrument(span),
            ));
        }
        for handle in handles {
            if let Err(err) = handle.await {
                tracing::warn!(error = %err, "burst task join failed");
            }
        }
    })
}

fn emit_field_spike(screen: &str) {
    for spike_index in 0..30 {
        tracing::warn!(
            app_screen_name = %screen,
            cart_tier = "free",
            spike_index,
            "slow render observed"
        );
    }
}

fn emit_rogue_log() {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        tracing::error!(
            evidence_gap = true,
            "orphan diagnostic without trace context"
        );
    });
}

async fn flood_blocking_pool(block_ms: u64, block_n: u32) {
    let capped_ms = block_ms.min(MAX_BLOCK_MS);
    let capped_n = block_n.min(MAX_BLOCK_N);
    tracing::warn!(
        requested_block_ms = block_ms,
        requested_block_n = block_n,
        block_ms = capped_ms,
        block_n = capped_n,
        "tokio blocking-pool saturation requested"
    );
    let mut handles = Vec::with_capacity(capped_n as usize);
    for i in 0..capped_n {
        BLOCKING_POOL_DEPTH.fetch_add(1, Ordering::Relaxed);
        record_blocking_pool_depth();
        handles.push(tokio::task::spawn_blocking(move || {
            std::thread::sleep(std::time::Duration::from_millis(capped_ms));
            BLOCKING_POOL_DEPTH.fetch_sub(1, Ordering::Relaxed);
            record_blocking_pool_depth();
            tracing::debug!(
                task = i,
                block_ms = capped_ms,
                "blocking-pool task completed"
            );
        }));
    }
    for handle in handles {
        if let Err(err) = handle.await {
            tracing::warn!(error = %err, "blocking-pool task join failed");
        }
    }
}

fn record_blocking_pool_depth() {
    static GAUGE: OnceLock<opentelemetry::metrics::Gauge<u64>> = OnceLock::new();
    let gauge = GAUGE.get_or_init(|| {
        opentelemetry::global::meter("playground.runtime")
            .u64_gauge(semconv::TOKIO_RUNTIME_BLOCKING_POOL_DEPTH)
            .with_description("Checkout spawn_blocking tasks in flight for the A22 saturation demo")
            .build()
    });
    gauge.record(BLOCKING_POOL_DEPTH.load(Ordering::Relaxed), &[]);
}

/// B3: bounded retry with a per-attempt deadline around the pricing call.
#[tracing::instrument]
async fn quote_with_retry(
    sku: &str,
    quantity: u32,
    retry: u32,
    timeout_ms: u64,
    delay_ms: u32,
) -> anyhow::Result<(u64, String)> {
    let attempts = retry.saturating_add(1);
    let mut last: anyhow::Result<(u64, String)> = Err(anyhow::anyhow!("no attempt"));
    for attempt in 1..=attempts {
        match pricing_attempt(sku, quantity, timeout_ms, delay_ms, attempt).await {
            Ok(ok) => return Ok(ok),
            Err(err) => {
                tracing::warn!(attempt, error = %err, "pricing attempt failed");
                last = Err(err);
            }
        }
    }
    last
}

async fn pricing_attempt(
    sku: &str,
    quantity: u32,
    timeout_ms: u64,
    delay_ms: u32,
    attempt: u32,
) -> anyhow::Result<(u64, String)> {
    let span = tracing::info_span!(
        "pricing.attempt",
        otel.kind = semconv::SPAN_KIND_CLIENT,
        attempt,
        timeout_ms,
        "rpc.system" = "grpc",
        "rpc.service" = "playground.pricing.v1.Pricing",
        "rpc.method" = "Quote",
        "rpc.grpc.status_code" = tracing::field::Empty,
    );
    quote(sku, quantity, timeout_ms, delay_ms)
        .instrument(span)
        .await
}

async fn quote(
    sku: &str,
    quantity: u32,
    timeout_ms: u64,
    delay_ms: u32,
) -> anyhow::Result<(u64, String)> {
    let endpoint =
        std::env::var("PRICING_ENDPOINT").unwrap_or_else(|_| "http://pricing:50051".into());
    let mut client = PricingClient::connect(endpoint).await?;
    let mut request = tonic::Request::new(QuoteRequest {
        sku: sku.to_string(),
        quantity,
        delay_ms,
        fail_at: 0,
    });
    request.set_timeout(std::time::Duration::from_millis(timeout_ms.max(1)));
    // Inject traceparent/tracestate/baggage into the gRPC metadata.
    playground_telemetry::inject_grpc_metadata(request.metadata_mut());
    let response = match client.quote(request).await {
        Ok(response) => {
            tracing::Span::current().record("rpc.grpc.status_code", 0_i64);
            response.into_inner()
        }
        Err(status) => {
            record_grpc_error(&status);
            return Err(anyhow::anyhow!(
                "pricing gRPC {:?}: {}",
                status.code(),
                status.message()
            ));
        }
    };
    Ok((response.total_minor, response.currency))
}

fn record_grpc_error(status: &tonic::Status) {
    let code = status.code();
    tracing::Span::current().record("rpc.grpc.status_code", grpc_code_number(code));
    if code == Code::DeadlineExceeded {
        playground_telemetry::mark_span_error("deadline_exceeded");
    } else {
        playground_telemetry::mark_span_error("grpc_error");
    }
    tracing::warn!(
        "rpc.grpc.status_code" = grpc_code_number(code),
        code = ?code,
        message = status.message(),
        "pricing gRPC status"
    );
}

fn grpc_code_number(code: Code) -> i64 {
    match code {
        Code::Ok => 0,
        Code::Cancelled => 1,
        Code::Unknown => 2,
        Code::InvalidArgument => 3,
        Code::DeadlineExceeded => 4,
        Code::NotFound => 5,
        Code::AlreadyExists => 6,
        Code::PermissionDenied => 7,
        Code::ResourceExhausted => 8,
        Code::FailedPrecondition => 9,
        Code::Aborted => 10,
        Code::OutOfRange => 11,
        Code::Unimplemented => 12,
        Code::Internal => 13,
        Code::Unavailable => 14,
        Code::DataLoss => 15,
        Code::Unauthenticated => 16,
    }
}

/// A7: consume the pricing server-stream (a long-lived streaming CLIENT span).
async fn quote_stream(headers: HeaderMap, Query(p): Query<CheckoutParams>) -> Json<Value> {
    let span = tracing::info_span!(
        "quote_stream",
        otel.kind = semconv::SPAN_KIND_SERVER,
        "rpc.system" = "grpc",
        "rpc.service" = "playground.Pricing",
        "rpc.method" = "QuoteStream",
    );
    playground_telemetry::set_parent_from_headers(&span, &headers);
    quote_stream_inner(p).instrument(span).await
}

async fn quote_stream_inner(p: CheckoutParams) -> Json<Value> {
    use tokio_stream::StreamExt as _;
    let endpoint =
        std::env::var("PRICING_ENDPOINT").unwrap_or_else(|_| "http://pricing:50051".into());
    let (count, cancelled, stream_error) = async {
        let mut client = PricingClient::connect(endpoint).await.ok()?;
        let mut request = tonic::Request::new(QuoteRequest {
            sku: p.sku.clone(),
            quantity: p.quantity,
            delay_ms: p.delay_ms,
            fail_at: p.fail_at,
        });
        playground_telemetry::inject_grpc_metadata(request.metadata_mut());
        let mut stream = client.quote_stream(request).await.ok()?.into_inner();
        let mut n = 0u32;
        let cancel_at = (p.cancel_ms > 0)
            .then(|| std::time::Instant::now() + std::time::Duration::from_millis(p.cancel_ms));
        let mut cancelled = false;
        let mut stream_error = None;
        loop {
            let item = if let Some(cancel_at) = cancel_at {
                let now = std::time::Instant::now();
                if now >= cancel_at {
                    cancelled = true;
                    tracing::warn!(
                        cancel_ms = p.cancel_ms,
                        count = n,
                        "pricing stream cancelled by client"
                    );
                    break;
                }
                match tokio::time::timeout(cancel_at.saturating_duration_since(now), stream.next())
                    .await
                {
                    Ok(item) => item,
                    Err(_) => {
                        cancelled = true;
                        tracing::warn!(
                            cancel_ms = p.cancel_ms,
                            count = n,
                            "pricing stream cancelled by client"
                        );
                        break;
                    }
                }
            } else {
                stream.next().await
            };
            match item {
                Some(Ok(_item)) => {
                    n += 1;
                    tracing::info!(
                        "rpc.message.type" = "RECEIVED",
                        "rpc.message.id" = i64::from(n),
                        "rpc.message"
                    );
                }
                Some(Err(status)) => {
                    playground_telemetry::mark_span_error("stream_failed");
                    tracing::error!(
                        code = ?status.code(),
                        message = status.message(),
                        received = n,
                        "pricing stream failed"
                    );
                    stream_error = Some(status.message().to_string());
                    break;
                }
                None => break,
            }
        }
        Some((n, cancelled, stream_error))
    }
    .await
    .unwrap_or((0, false, Some("stream unavailable".to_string())));
    tracing::info!(sku = %p.sku, count, cancelled, "consumed pricing stream");
    Json(json!({
        "sku": p.sku,
        "streamed_quotes": count,
        "cancelled": cancelled,
        "error": stream_error,
    }))
}

#[tracing::instrument(fields(otel.kind = semconv::SPAN_KIND_CLIENT))]
async fn reserve(sku: &str, quantity: u32) -> anyhow::Result<Value> {
    let base = std::env::var("INVENTORY_URL").unwrap_or_else(|_| "http://inventory:8089".into());
    let url = format!("{base}/reserve?sku={sku}&quantity={quantity}");
    Ok(playground_telemetry::traced_get(&url)
        .await?
        .json::<Value>()
        .await?)
}

#[tracing::instrument(fields(otel.kind = semconv::SPAN_KIND_CLIENT))]
async fn recommend(sku: &str) -> anyhow::Result<Value> {
    let base =
        std::env::var("RECOMMENDATION_URL").unwrap_or_else(|_| "http://recommendation:8090".into());
    let url = format!("{base}/recommend?sku={sku}");
    Ok(playground_telemetry::traced_get(&url)
        .await?
        .json::<Value>()
        .await?)
}

fn cors_layer() -> CorsLayer {
    let origin = std::env::var("WEB_ORIGIN")
        .ok()
        .and_then(|origin| origin.parse::<HeaderValue>().ok())
        .map(AllowOrigin::exact)
        .unwrap_or_else(|| {
            // Local lab stack: mirror request origin so browser trace headers
            // can cross from the demo UI to checkout without per-port config.
            AllowOrigin::mirror_request()
        });
    CorsLayer::new()
        .allow_origin(origin)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([
            header::CONTENT_TYPE,
            HeaderName::from_static("traceparent"),
            HeaderName::from_static("tracestate"),
            HeaderName::from_static("baggage"),
        ])
}

fn app() -> Router {
    Router::new()
        .route("/checkout", get(checkout))
        .route("/quote-stream", get(quote_stream))
        .route("/healthz", get(|| async { "ok" }))
        .layer(cors_layer())
        .layer(axum::middleware::from_fn(
            playground_telemetry::http_server_observability,
        ))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("checkout")?;
    let app = app();
    let addr = std::env::var("CHECKOUT_ADDR").unwrap_or_else(|_| "0.0.0.0:8088".into());
    tracing::info!(%addr, "checkout HTTP listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[test]
    fn payment_error_has_the_shared_cross_language_identity() {
        assert_eq!(PaymentError.to_string(), "PaymentError: payment failed");
    }

    #[test]
    fn clamp_shape_bounds_fan_depth_and_estimated_spans() {
        assert_eq!(clamp_shape(0, 3), (0, 0));
        assert_eq!(clamp_shape(15, 2), (15, 2));

        let (fan, depth) = clamp_shape(50, 20);
        assert!(fan <= MAX_BURST_FAN);
        assert_eq!(depth, MAX_BURST_DEPTH);
        assert!(estimated_burst_spans(fan, depth) <= MAX_BURST_SPANS);
    }

    #[test]
    fn compare_variant_defaults_to_v1_and_accepts_v2() {
        assert_eq!(compare_variant(None), "v1");
        assert_eq!(compare_variant(Some("v1")), "v1");
        assert_eq!(compare_variant(Some("v2")), "v2");
        assert_eq!(compare_variant(Some("other")), "v1");
    }

    #[tokio::test]
    async fn exposes_health_without_downstream_dependencies() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("health request"),
            )
            .await
            .expect("health response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn returns_the_shared_payment_error_without_downstream_calls() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/checkout?fail=1")
                    .body(Body::empty())
                    .expect("checkout failure request"),
            )
            .await
            .expect("checkout failure response");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        assert!(
            std::str::from_utf8(&body)
                .expect("UTF-8 response")
                .contains("payment failed")
        );
    }

    #[tokio::test]
    async fn serves_health_over_a_real_loopback_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind checkout listener");
        let address = listener.local_addr().expect("checkout listener address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app()).await.expect("serve checkout");
        });

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            reqwest::get(format!("http://{address}/healthz")),
        )
        .await
        .expect("checkout health timeout")
        .expect("checkout health response");
        assert_eq!(response.status(), StatusCode::OK);
        server.abort();
    }
}
