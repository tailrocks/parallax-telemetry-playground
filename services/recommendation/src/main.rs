//! Recommendation HTTP service — related SKUs (cache-backed in the full design).
//! Chaos: ?leak=<n> grows a process-held buffer to emulate a cache/memory leak
//! (B6) and adds latency, so the slow degradation is visible over repeated calls.
use axum::http::HeaderMap;
use axum::{Json, Router, extract::Query, routing::get};
use opentelemetry::metrics::{Counter, Gauge};
use opentelemetry::{KeyValue, global};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::Instrument;

/// Default in-process recommendation cache TTL.
pub const REC_TTL: Duration = Duration::from_secs(30);
/// Shortest TTL allowed by `ttl_ms`, keeping demos visible but nonzero.
pub const MIN_TTL_MS: u64 = 100;
/// Longest TTL allowed by `ttl_ms`, bounding stale demo data.
pub const MAX_TTL_MS: u64 = 300_000;
/// Max parallel internal workers for the intentionally unprotected stampede.
pub const MAX_STAMPEDE: usize = 100;
const COMPUTE_LATENCY: Duration = Duration::from_millis(80);

#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheEntry {
    value: Vec<String>,
    inserted: Instant,
}

struct CacheMetrics {
    hits: Counter<u64>,
    misses: Counter<u64>,
    size: Gauge<u64>,
}

fn leak_store() -> &'static Mutex<Vec<Vec<u8>>> {
    static STORE: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

fn rec_cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_metrics() -> &'static CacheMetrics {
    static METRICS: OnceLock<CacheMetrics> = OnceLock::new();
    METRICS.get_or_init(|| {
        let meter = global::meter("playground.cache");
        CacheMetrics {
            hits: meter
                .u64_counter("cache.hits")
                .with_description("Recommendation cache hits")
                .build(),
            misses: meter
                .u64_counter("cache.misses")
                .with_description("Recommendation cache misses")
                .build(),
            size: meter
                .u64_gauge("cache.size")
                .with_description("Recommendation cache entries")
                .build(),
        }
    })
}

#[derive(Deserialize)]
struct Recommend {
    sku: String,
    #[serde(default)]
    leak: usize,
    /// B13: slow "asset"/response latency.
    #[serde(default)]
    slow: u64,
    /// A26: `cache=0` bypasses the in-process TTL cache.
    #[serde(default)]
    cache: Option<u8>,
    /// A26: TTL override, clamped by MIN_TTL_MS/MAX_TTL_MS.
    #[serde(default)]
    ttl_ms: Option<u64>,
    /// A26: invalidates the SKU and spawns unprotected parallel lookups.
    #[serde(default)]
    stampede: usize,
}

#[derive(Clone, Debug)]
struct RecommendationResult {
    recommended: Vec<String>,
    hit: bool,
    cache_size: usize,
}

fn cache_enabled(value: Option<u8>) -> bool {
    value != Some(0)
}

fn ttl_from_ms(value: Option<u64>) -> Duration {
    Duration::from_millis(
        value
            .unwrap_or(REC_TTL.as_millis() as u64)
            .clamp(MIN_TTL_MS, MAX_TTL_MS),
    )
}

fn stampede_from(value: usize) -> usize {
    value.min(MAX_STAMPEDE)
}

fn lookup_cached(
    cache: &mut HashMap<String, CacheEntry>,
    sku: &str,
    now: Instant,
    ttl: Duration,
) -> Option<Vec<String>> {
    if let Some(entry) = cache.get(sku)
        && now.duration_since(entry.inserted) <= ttl
    {
        return Some(entry.value.clone());
    }
    cache.remove(sku);
    None
}

fn insert_cached(
    cache: &mut HashMap<String, CacheEntry>,
    sku: String,
    value: Vec<String>,
    now: Instant,
) -> usize {
    cache.insert(
        sku,
        CacheEntry {
            value,
            inserted: now,
        },
    );
    cache.len()
}

fn invalidate_cached(sku: &str) {
    rec_cache().lock().unwrap().remove(sku);
}

fn current_cache_size() -> usize {
    rec_cache().lock().unwrap().len()
}

fn record_cache_metrics(hit: bool, size: usize) {
    let metrics = cache_metrics();
    let attrs = [KeyValue::new("cache.name", "recommendations")];
    if hit {
        metrics.hits.add(1, &attrs);
    } else {
        metrics.misses.add(1, &attrs);
    }
    metrics.size.record(size as u64, &attrs);
}

async fn recommend(headers: HeaderMap, Query(p): Query<Recommend>) -> Json<Value> {
    let span = tracing::info_span!("recommend", otel.kind = "server");
    playground_telemetry::set_parent_from_headers(&span, &headers);
    recommend_inner(p).instrument(span).await
}

async fn recommend_inner(p: Recommend) -> Json<Value> {
    if p.slow > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(p.slow)).await;
    }
    if p.leak > 0 {
        let mut store = leak_store().lock().unwrap();
        let mut bytes = vec![0u8; p.leak * 1024];
        // Touch each page so the cgroup sees real RSS instead of lazily shared
        // zero pages. The buffer is never freed, so repeated calls OOM under
        // the demo limits overlay.
        for i in (0..bytes.len()).step_by(4096) {
            bytes[i] = (i % 251) as u8;
        }
        store.push(bytes);
        tracing::warn!(kb = p.leak, held = store.len(), "cache leak (chaos)");
    }

    let cache_enabled = cache_enabled(p.cache);
    let ttl = ttl_from_ms(p.ttl_ms);
    let stampede = stampede_from(p.stampede);
    let result = if stampede > 0 {
        stampede_recommendations(p.sku.clone(), stampede, cache_enabled, ttl).await
    } else {
        lookup_or_compute(&p.sku, cache_enabled, ttl).await
    };

    tracing::info!(
        sku = %p.sku,
        count = result.recommended.len(),
        cache.hit = result.hit,
        cache.enabled = cache_enabled,
        cache.size = result.cache_size,
        ttl_ms = ttl.as_millis() as u64,
        stampede,
        "recommended"
    );
    Json(json!({
        "sku": p.sku,
        "recommended": result.recommended,
        "cache_hit": result.hit,
        "cache_size": result.cache_size,
        "stampede_workers": stampede,
    }))
}

async fn lookup_or_compute(sku: &str, cache_enabled: bool, ttl: Duration) -> RecommendationResult {
    if cache_enabled {
        let (cached, cache_size) = {
            let mut cache = rec_cache().lock().unwrap();
            let cached = lookup_cached(&mut cache, sku, Instant::now(), ttl);
            let cache_size = cache.len();
            (cached, cache_size)
        };
        if let Some(recommended) = cached {
            record_cache_metrics(true, cache_size);
            return RecommendationResult {
                recommended,
                hit: true,
                cache_size,
            };
        }
    }

    let recommended = compute_recommendations(sku).await;
    let cache_size = if cache_enabled {
        let mut cache = rec_cache().lock().unwrap();
        insert_cached(
            &mut cache,
            sku.to_string(),
            recommended.clone(),
            Instant::now(),
        )
    } else {
        current_cache_size()
    };
    record_cache_metrics(false, cache_size);
    RecommendationResult {
        recommended,
        hit: false,
        cache_size,
    }
}

async fn stampede_recommendations(
    sku: String,
    workers: usize,
    cache_enabled: bool,
    ttl: Duration,
) -> RecommendationResult {
    invalidate_cached(&sku);
    // A26 intentionally has no single-flight protection. The parallel miss
    // burst is the thundering-herd demo, so do not collapse these workers.
    tracing::warn!(sku = %sku, workers, "cache stampede requested");
    let mut handles = Vec::with_capacity(workers);
    for worker in 0..workers {
        let worker_sku = sku.clone();
        handles.push(tokio::spawn(
            async move { lookup_or_compute(&worker_sku, cache_enabled, ttl).await }.instrument(
                tracing::info_span!(
                    "stampede_worker",
                    worker,
                    sku = %sku
                ),
            ),
        ));
    }

    let mut last = None;
    for handle in handles {
        match handle.await {
            Ok(result) => last = Some(result),
            Err(err) => tracing::warn!(error = %err, "stampede worker join failed"),
        }
    }
    match last {
        Some(result) => result,
        None => lookup_or_compute(&sku, cache_enabled, ttl).await,
    }
}

async fn compute_recommendations(sku: &str) -> Vec<String> {
    let span = tracing::info_span!("compute_recommendations", sku = %sku, otel.kind = "internal");
    async move {
        tokio::time::sleep(COMPUTE_LATENCY).await;
        vec![format!("{sku}-ACCESSORY"), "WIDGET-2".to_string()]
    }
    .instrument(span)
    .await
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let telemetry = playground_telemetry::init("recommendation")?;
    let app = Router::new()
        .route("/recommend", get(recommend))
        .route("/healthz", get(|| async { "ok" }));
    let addr = std::env::var("ADDR").unwrap_or_else(|_| "0.0.0.0:8090".into());
    tracing::info!(%addr, "recommendation HTTP listening");
    axum::serve(tokio::net::TcpListener::bind(&addr).await?, app)
        .with_graceful_shutdown(playground_telemetry::shutdown_signal())
        .await?;
    telemetry.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_helper_hits_and_expires() {
        let now = Instant::now();
        let mut cache = HashMap::new();
        insert_cached(
            &mut cache,
            "WIDGET-1".to_string(),
            vec!["WIDGET-1-ACCESSORY".to_string()],
            now,
        );

        assert_eq!(
            lookup_cached(
                &mut cache,
                "WIDGET-1",
                now + Duration::from_millis(50),
                Duration::from_millis(100)
            ),
            Some(vec!["WIDGET-1-ACCESSORY".to_string()])
        );
        assert_eq!(
            lookup_cached(
                &mut cache,
                "WIDGET-1",
                now + Duration::from_millis(101),
                Duration::from_millis(100)
            ),
            None
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_bypass_ttl_and_stampede_clamps() {
        assert!(!cache_enabled(Some(0)));
        assert!(cache_enabled(None));
        assert_eq!(ttl_from_ms(Some(1)), Duration::from_millis(MIN_TTL_MS));
        assert_eq!(
            ttl_from_ms(Some(MAX_TTL_MS + 1)),
            Duration::from_millis(MAX_TTL_MS)
        );
        assert_eq!(stampede_from(MAX_STAMPEDE + 10), MAX_STAMPEDE);
    }
}
