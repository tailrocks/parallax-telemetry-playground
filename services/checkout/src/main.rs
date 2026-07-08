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

use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::IntoResponse;
use axum::{Json, Router, extract::Query, routing::get};
use open_feature::EvaluationContext;
use open_feature::provider::FeatureProvider;
use open_feature_flagd::{FlagdOptions, FlagdProvider, ResolverType};
use playground_proto::pricing::v1::QuoteRequest;
use playground_proto::pricing::v1::pricing_client::PricingClient;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OnceCell;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::Instrument;

static FLAGD_PROVIDER: OnceCell<FlagdProvider> = OnceCell::const_new();

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
    /// B3: retry the pricing call up to N times with a per-attempt deadline.
    #[serde(default)]
    retry: u32,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
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
    /// B18: emit a span event with a deliberately skewed timestamp.
    #[serde(default, deserialize_with = "de_flag")]
    skew: bool,
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
    let span = tracing::info_span!("checkout", otel.kind = "server");
    playground_telemetry::set_parent_from_headers(&span, &headers);
    checkout_inner(p).instrument(span).await
}

async fn checkout_inner(p: CheckoutParams) -> impl IntoResponse {
    let payment_failure_flag = feature_flag("paymentFailure", "PAYMENT_FAILURE").await;
    let slow_query_flag = feature_flag("slowQuery", "SLOW_QUERY").await;
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
    // A10: business context as baggage (propagated downstream in the full design).
    if let Some(tenant) = &p.tenant {
        tracing::info!(tenant = %tenant, user.tier = %p.tier, "baggage business context");
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
    if p.canary || env_flag("CANARY") {
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
        // B18: a span event timestamped far in the past (clock skew across hops).
        let skewed = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        let skewed = skewed
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        tracing::warn!(skewed_unix_s = skewed, "clock-skew event (1h in the past)");
    }
    // B12: release-attributed regression — RELEASE=v2 fails (vs v1 clean).
    let release_regressed = std::env::var("RELEASE").as_deref() == Ok("v2");
    if p.fail || payment_failure_flag || release_regressed {
        // B1/B12: deliberate failure → error issue + ERROR span status.
        playground_telemetry::mark_span_error("payment_failure");
        tracing::error!(sku = %p.sku, payment_failure_flag, release_regressed, "payment failure (chaos)");
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

    // B9: N+1 — fire N extra sequential inventory calls (a classic hotspot).
    for i in 0..p.n1 {
        let _ = reserve(&p.sku, 1).await;
        tracing::debug!(i, "n+1 inventory call");
    }

    let pricing = quote_with_retry(&p.sku, p.quantity, p.retry, p.timeout_ms).await;
    let inventory = reserve(&p.sku, p.quantity).await;
    let recommendation = recommend(&p.sku).await;

    match pricing {
        Ok((total, currency)) => {
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
            tracing::error!(error = %err, "pricing call failed");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": err.to_string() })),
            )
        }
    }
}

async fn feature_flag(flag_key: &'static str, env_name: &'static str) -> bool {
    let env_override = env_flag(env_name);
    let mut provider_name = "flagd";
    let mut provider_value = false;
    let mut variant = "off".to_string();
    let mut error = String::new();

    match flagd_provider().await {
        Ok(provider) => match provider
            .resolve_bool_value(flag_key, &EvaluationContext::default())
            .await
        {
            Ok(details) => {
                provider_value = details.value;
                variant = details
                    .variant
                    .unwrap_or_else(|| if provider_value { "on" } else { "off" }.to_string());
            }
            Err(err) => {
                provider_name = "env";
                error = format!("{err:?}");
            }
        },
        Err(err) => {
            provider_name = "env";
            error = err.to_string();
        }
    }

    let effective = provider_value || env_override;
    if env_override {
        variant = "env-on".to_string();
    }
    tracing::info!(
        "feature_flag.key" = flag_key,
        "feature_flag.provider_name" = provider_name,
        "feature_flag.variant" = %variant,
        "feature_flag.value" = effective,
        "feature_flag.env_override" = env_override,
        "feature_flag.error" = %error,
        "feature_flag.evaluation"
    );
    effective
}

async fn flagd_provider() -> anyhow::Result<&'static FlagdProvider> {
    FLAGD_PROVIDER
        .get_or_try_init(|| async {
            FlagdProvider::new(FlagdOptions {
                resolver_type: ResolverType::Rpc,
                ..Default::default()
            })
            .await
            .map_err(anyhow::Error::new)
        })
        .await
}

/// B3: bounded retry with a per-attempt deadline around the pricing call.
#[tracing::instrument]
async fn quote_with_retry(
    sku: &str,
    quantity: u32,
    retry: u32,
    timeout_ms: u64,
) -> anyhow::Result<(u64, String)> {
    let attempts = retry.saturating_add(1);
    let mut last: anyhow::Result<(u64, String)> = Err(anyhow::anyhow!("no attempt"));
    for attempt in 1..=attempts {
        let deadline = std::time::Duration::from_millis(timeout_ms);
        match tokio::time::timeout(deadline, quote(sku, quantity)).await {
            Ok(Ok(ok)) => return Ok(ok),
            Ok(Err(err)) => {
                tracing::warn!(attempt, error = %err, "pricing attempt failed");
                last = Err(err);
            }
            Err(_) => {
                tracing::warn!(attempt, timeout_ms, "pricing attempt timed out");
                last = Err(anyhow::anyhow!("pricing deadline exceeded"));
            }
        }
    }
    last
}

#[tracing::instrument(fields(otel.kind = "client"))]
async fn quote(sku: &str, quantity: u32) -> anyhow::Result<(u64, String)> {
    let endpoint =
        std::env::var("PRICING_ENDPOINT").unwrap_or_else(|_| "http://pricing:50051".into());
    let mut client = PricingClient::connect(endpoint).await?;
    let mut request = tonic::Request::new(QuoteRequest {
        sku: sku.to_string(),
        quantity,
    });
    // Inject traceparent/tracestate/baggage into the gRPC metadata.
    playground_telemetry::inject_grpc_metadata(request.metadata_mut());
    let response = client.quote(request).await?.into_inner();
    Ok((response.total_minor, response.currency))
}

/// A7: consume the pricing server-stream (a long-lived streaming CLIENT span).
async fn quote_stream(headers: HeaderMap, Query(p): Query<CheckoutParams>) -> Json<Value> {
    let span = tracing::info_span!("quote_stream", otel.kind = "server");
    playground_telemetry::set_parent_from_headers(&span, &headers);
    quote_stream_inner(p).instrument(span).await
}

async fn quote_stream_inner(p: CheckoutParams) -> Json<Value> {
    use tokio_stream::StreamExt as _;
    let endpoint =
        std::env::var("PRICING_ENDPOINT").unwrap_or_else(|_| "http://pricing:50051".into());
    let count = async {
        let mut client = PricingClient::connect(endpoint).await.ok()?;
        let mut request = tonic::Request::new(QuoteRequest {
            sku: p.sku.clone(),
            quantity: p.quantity,
        });
        playground_telemetry::inject_grpc_metadata(request.metadata_mut());
        let mut stream = client.quote_stream(request).await.ok()?.into_inner();
        let mut n = 0u32;
        while let Some(Ok(_item)) = stream.next().await {
            n += 1;
        }
        Some(n)
    }
    .await
    .unwrap_or(0);
    tracing::info!(sku = %p.sku, count, "consumed pricing stream");
    Json(json!({ "sku": p.sku, "streamed_quotes": count }))
}

#[tracing::instrument(fields(otel.kind = "client"))]
async fn reserve(sku: &str, quantity: u32) -> anyhow::Result<Value> {
    let base = std::env::var("INVENTORY_URL").unwrap_or_else(|_| "http://inventory:8089".into());
    let url = format!("{base}/reserve?sku={sku}&quantity={quantity}");
    Ok(playground_telemetry::traced_get(&url)
        .await?
        .json::<Value>()
        .await?)
}

#[tracing::instrument(fields(otel.kind = "client"))]
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("checkout")?;
    let app = Router::new()
        .route("/checkout", get(checkout))
        .route("/quote-stream", get(quote_stream))
        .route("/healthz", get(|| async { "ok" }))
        .layer(cors_layer());
    let addr = std::env::var("CHECKOUT_ADDR").unwrap_or_else(|_| "0.0.0.0:8088".into());
    tracing::info!(%addr, "checkout HTTP listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}
