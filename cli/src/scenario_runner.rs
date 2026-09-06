//! Rust-owned scenario entrypoints.
//!
//! The public interface is a semantic group:case name. Legacy A/B/C/T/L/M
//! labels remain only in synthetic payloads that exercise rendering behavior.

use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, bail, ensure};
use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::{Router, serve};
use futures::StreamExt;
use hmac::{Hmac, Mac};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use reqwest::{Client, Method, StatusCode, Url, header::HeaderMap};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinSet;

use crate::shapes;
use playground_telemetry::invocation;

/// Stable public registry. Every name has exactly one group and one semantic
/// snake_case case. Fixture IDs are selected internally below.
pub(crate) const SCENARIO_NAMES: &[&str] = &[
    "commerce:checkout_saga",
    "metrics:exemplars",
    "browser:rum_error",
    "graphql:batching_errors",
    "database:price_subscription",
    "grpc:pricing_stream",
    "messaging:java_fulfillment_replay",
    "logs:field_spike",
    "propagation:baggage",
    "messaging:checkout_outbox",
    "messaging:seeded_order_replay",
    "cli:checkout_invocation",
    "deploy:release_regression",
    "feature_flags:checkout_variants",
    "security:redaction_canary",
    "traces:wide_trace",
    "feature_flags:topology_compare",
    "messaging:batch_fanin",
    "runtime:request_saturation",
    "grpc:storefront_pricing",
    "graphql:storefront_catalog",
    "postgres:query_pressure",
    "cache:recommendation_stampede",
    "agent:execution_stack",
    "browser:rum_journey",
    "events:typed_business_events",
    "metrics:request_shapes",
    "errors:handled_unhandled",
    "messaging:poison_retry",
    "failures:inventory",
    "runtime:cpu_pressure",
    "memory:cache_leak",
    "postgres:lock_contention",
    "recommendation:slow_query",
    "browser:rage_click",
    "load:checkout",
    "failures:payment_latency",
    "failures:checkout_chaos",
    "grpc:deadline_retry",
    "alerts:error_rate_breach",
    "alerts:p95_breach",
    "alerts:recovery",
    "failures:provider_degradation",
    "cron:outcomes",
    "cron:duplicate_missed",
    "jvm:memory_pressure",
    "container:recommendation_oom_probe",
    "messaging:orphan_consumer",
    "sampling:low_sample_gap",
    "logs:trace_correlation",
    "traces:deep",
    "traces:wide",
    "traces:multi_root",
    "traces:orphan",
    "traces:clock_skew",
    "traces:zero_duration",
    "traces:cross_links",
    "traces:long_names",
    "traces:events",
    "logs:burst",
    "logs:bodies",
    "logs:patterns",
    "metrics:shapes",
    "metrics:labels",
    "attributes:bounded",
    "issues:burst",
    "issues:multi_language",
    "protocols:grpc_errors",
    "protocols:grpc_stream",
    "protocols:graphql_errors",
    "protocols:rabbitmq_lag",
    "journeys:happy_path",
    "journeys:error_path",
    "journeys:outside_screen",
    "journeys:reattach",
    "journeys:parallel",
    "ecosystem:external_edge",
    "ecosystem:full",
    "product:issue_context",
    "product:invocation_lifecycle",
    "product:live_tail",
    "product:alerting",
    "product:saved_state",
    "product:github_ingest",
    "product:agent_session",
    "sentry:envelopes",
    "product:lifecycle_ops",
    "security:redaction_egress",
    "product:ui_agent_verify",
];

pub(crate) fn semantic_names() -> &'static [&'static str] {
    SCENARIO_NAMES
}

pub(crate) async fn run(args: Vec<String>) -> anyhow::Result<i32> {
    let Some(name) = args.first().map(String::as_str) else {
        bail!("usage: playground scenario <group:semantic_case> [arguments]");
    };
    if name == "list" {
        for scenario in SCENARIO_NAMES {
            println!("{scenario}");
        }
        return Ok(0);
    }
    ensure!(
        SCENARIO_NAMES.contains(&name) || name == "corpus:all",
        "unknown semantic scenario: {name}; use playground scenario list"
    );
    let extra = &args[1..];
    run_named(name, extra).await
}

async fn run_named(name: &str, extra: &[String]) -> anyhow::Result<i32> {
    println!("scenario {name}");

    match name {
        "commerce:checkout_saga" => checkout_saga().await,
        "messaging:checkout_outbox" => checkout_outbox().await,
        "metrics:exemplars" => catalog_queries("exemplars", positive_env("A2_REQUESTS", 12)?).await,
        "graphql:batching_errors" => graphql_shapes().await,
        "database:price_subscription" => run_bun_script("scenarios/a7-subscription.ts", &[]).await,
        "grpc:pricing_stream" | "protocols:grpc_stream" => pricing_stream().await,
        "messaging:java_fulfillment_replay" | "messaging:seeded_order_replay" => {
            seeded_order_replay().await
        }
        "logs:field_spike" => run_shape("l-patterns").await,
        "propagation:baggage" => baggage_checkout().await,
        "cli:checkout_invocation" => run_current(&[]).await,
        "deploy:release_regression" => release_regression().await,
        "feature_flags:checkout_variants" => feature_flag_variants().await,
        "feature_flags:topology_compare" => feature_flag_topology_compare().await,
        "security:redaction_canary" => sentry_canary().await,
        "traces:wide_trace" => run_shape("t-wide").await,
        "messaging:batch_fanin" => synthetic_batch_fanin().await,
        "messaging:poison_retry" => synthetic_poison_retry().await,
        "messaging:orphan_consumer" => synthetic_orphan_consumer().await,
        "runtime:request_saturation" => checkout_pressure().await,
        "runtime:cpu_pressure" => delayed_checkout_burst("runtime:cpu_pressure", "B5").await,
        "postgres:lock_contention" => {
            delayed_checkout_concurrency("postgres:lock_contention").await
        }
        "failures:payment_latency" => payment_failure_latency().await,
        "failures:checkout_chaos" => checkout_chaos().await,
        "failures:provider_degradation" => provider_degradation().await,
        "sampling:low_sample_gap" => sampling_gap().await,
        "logs:trace_correlation" => checkout_probe(name).await,
        "errors:handled_unhandled" => handled_unhandled().await,
        "failures:inventory" => inventory_failure().await,
        "grpc:storefront_pricing" => storefront_graphql("pricing").await,
        "graphql:storefront_catalog" => storefront_graphql("catalog").await,
        "postgres:query_pressure" => inventory_pressure().await,
        "cache:recommendation_stampede" => recommendation_stampede().await,
        "memory:cache_leak" => recommendation_cache_leak().await,
        "recommendation:slow_query" => recommendation_slow_query().await,
        "agent:execution_stack" => execution_stack().await,
        "browser:rum_error" => {
            browser_test("e2e/compose.smoke.spec.ts", "browses, quotes, checks out").await
        }
        "browser:rum_journey" => browser_journey().await,
        "events:typed_business_events" => typed_events().await,
        "metrics:request_shapes" => request_metric_shapes().await,
        "browser:rage_click" => {
            browser_test(
                "e2e/journey.spec.ts",
                "orders reads durable status|analytics reads ClickHouse",
            )
            .await
        }
        "load:checkout" => k6_load(extra).await,
        "grpc:deadline_retry" => grpc_deadline_retry().await,
        "alerts:error_rate_breach" => sustained_checkout_failures().await,
        "alerts:p95_breach" => sustained_recommendation().await,
        "alerts:recovery" => recovery_traffic().await,
        "cron:outcomes" => run_current(&["cron", "weighted"]).await,
        "cron:duplicate_missed" => cron_suite().await,
        "jvm:memory_pressure" => jvm_memory_pressure().await,
        "container:recommendation_oom_probe" => container_oom(extra).await,
        "traces:deep" => run_shape("t-deep").await,
        "traces:wide" => run_shape("t-wide").await,
        "traces:multi_root" => run_shape("t-multiroot").await,
        "traces:orphan" => run_shape("t-orphan").await,
        "traces:clock_skew" => run_shape("t-skew").await,
        "traces:zero_duration" => run_shape("t-zero").await,
        "traces:cross_links" => run_shape("t-links").await,
        "traces:long_names" => run_shape("t-longnames").await,
        "traces:events" => run_shape("t-events").await,
        "logs:burst" => run_shape("l-burst").await,
        "logs:bodies" => run_shape("l-bodies").await,
        "logs:patterns" => run_shape("l-patterns").await,
        "metrics:shapes" => run_shape("m-shapes").await,
        "metrics:labels" => run_shape("m-labels").await,
        "attributes:bounded" => run_shape("f-attrs").await,
        "issues:burst" => run_shape("e-burst").await,
        "issues:multi_language" => run_shape("e-multi-lang").await,
        "protocols:grpc_errors" => grpc_error_corpus().await,
        "protocols:graphql_errors" => graphql_shapes().await,
        "protocols:rabbitmq_lag" => synthetic_poison_retry().await,
        "journeys:happy_path" => journey(&["--seconds", "6"]).await,
        "journeys:error_path" => expected_journey_failure().await,
        "journeys:outside_screen" => journey(&["--seconds", "6", "--outside-error"]).await,
        "journeys:reattach" => journey(&["--seconds", "9", "--reattach", "3"]).await,
        "journeys:parallel" => parallel_journeys().await,
        "ecosystem:external_edge" => run_shape("eco-external").await,
        "ecosystem:full" => ecosystem_full().await,
        "product:issue_context" => issue_context().await,
        "product:invocation_lifecycle" => invocation_lifecycle().await,
        "product:live_tail" => live_tail().await,
        "product:alerting" => alerting().await,
        "product:saved_state" => saved_state().await,
        "product:github_ingest" => github_ingest().await,
        "product:agent_session" => agent_session().await,
        "sentry:envelopes" => sentry_envelopes().await,
        "product:lifecycle_ops" => lifecycle_ops().await,
        "security:redaction_egress" => redaction_egress().await,
        "product:ui_agent_verify" => ui_agent_verify().await,
        "corpus:all" => corpus_all().await,
        _ => unreachable!("registry and dispatch must stay in sync"),
    }
}

async fn current_executable() -> anyhow::Result<PathBuf> {
    std::env::current_exe().context("cannot locate playground executable")
}

async fn run_current(args: &[&str]) -> anyhow::Result<i32> {
    run_program(
        current_executable().await?,
        args.iter().map(|arg| (*arg).to_owned()).collect(),
        None,
    )
    .await
}

async fn run_program(
    program: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
) -> anyhow::Result<i32> {
    run_program_with_env(program, args, cwd, &[]).await
}

async fn run_program_with_env(
    program: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: &[(&str, &str)],
) -> anyhow::Result<i32> {
    let mut command = Command::new(&program);
    command
        .args(&args)
        .envs(env.iter().copied())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command
        .status()
        .await
        .with_context(|| format!("failed to start {}", program.display()))?;
    let code = status.code().unwrap_or(1);
    if status.success() {
        Ok(0)
    } else {
        bail!("{} {:?} exited with {code}", program.display(), args)
    }
}

async fn capture_program(
    program: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
) -> anyhow::Result<(i32, String)> {
    capture_program_with_env(program, args, cwd, &[]).await
}

async fn capture_program_with_env(
    program: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    env: &[(&str, &str)],
) -> anyhow::Result<(i32, String)> {
    let mut command = Command::new(&program);
    command.args(&args).envs(env.iter().copied());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to start {}", program.display()))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.code().unwrap_or(1), text))
}

async fn run_current_allow_failure(args: &[&str]) -> anyhow::Result<(i32, String)> {
    run_current_allow_failure_with_env(args, &[]).await
}

async fn run_current_allow_failure_with_env(
    args: &[&str],
    env: &[(&str, &str)],
) -> anyhow::Result<(i32, String)> {
    let mut command = Command::new(current_executable().await?);
    command
        .args(args)
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .await
        .context("failed to start playground child")?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.code().unwrap_or(1), text))
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli is a workspace member")
        .to_path_buf()
}

fn url_env(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

fn nonempty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn env_or_fallback(primary: &str, fallback: &str) -> Option<String> {
    nonempty_env(primary).or_else(|| nonempty_env(fallback))
}

fn url_with_query(base: &str, path: &str, pairs: &[(String, String)]) -> anyhow::Result<String> {
    let mut url = Url::parse(&format!("{base}{path}"))?;
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

fn positive_env(name: &str, default: u64) -> anyhow::Result<u64> {
    let raw = nonempty_env(name).unwrap_or_else(|| default.to_string());
    let value = raw
        .parse::<u64>()
        .with_context(|| format!("{name} must be a positive integer: {raw}"))?;
    ensure!(value > 0, "{name} must be a positive integer: {raw}");
    Ok(value)
}

fn positive_float_env(name: &str, default: f64) -> anyhow::Result<f64> {
    let raw = nonempty_env(name).unwrap_or_else(|| default.to_string());
    let value = raw
        .parse::<f64>()
        .with_context(|| format!("{name} must be a positive number: {raw}"))?;
    ensure!(
        value.is_finite() && value > 0.0,
        "{name} must be a positive number: {raw}"
    );
    Ok(value)
}

fn configured_binary(env_name: &str, command: &str, sibling_name: &str) -> PathBuf {
    if let Ok(value) = std::env::var(env_name) {
        let value = value.trim();
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    if let Some(fallback) = repository_root()
        .parent()
        .map(|parent| {
            parent
                .join("parallax")
                .join("target")
                .join("debug")
                .join(sibling_name)
        })
        .filter(|path| path.is_file())
    {
        return fallback;
    }
    PathBuf::from(command)
}

fn parallax_bin() -> PathBuf {
    configured_binary("PARALLAX_BIN", "parallax", "parallax")
}

fn parallax_mcp() -> PathBuf {
    configured_binary("PARALLAX_MCP", "parallax-mcp", "parallax-mcp")
}

fn http_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(
            std::env::var("SCENARIO_HTTP_TIMEOUT_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(30),
        ))
        .build()?)
}

async fn request_json(
    method: Method,
    url: &str,
    headers: HeaderMap,
    body: Option<Value>,
) -> anyhow::Result<(StatusCode, Value)> {
    let client = http_client()?;
    let mut request = client.request(method, url).headers(headers);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("request failed: {url}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .context("failed to read HTTP response")?;
    let value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"raw": text}));
    Ok((status, value))
}

fn json_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        "application/json".parse().expect("valid header"),
    );
    headers
}

fn parallax_headers() -> anyhow::Result<HeaderMap> {
    let mut headers = json_headers();
    if let Some(token) = std::env::var("PARALLAX_API_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
    {
        headers.insert("authorization", format!("Bearer {token}").parse()?);
    }
    Ok(headers)
}

async fn parallax_graphql(query: &str) -> anyhow::Result<Value> {
    let base = url_env("PARALLAX_URL", "http://127.0.0.1:4000");
    let (status, body) = request_json(
        Method::POST,
        &format!("{base}/graphql"),
        parallax_headers()?,
        Some(json!({"query": query})),
    )
    .await?;
    ensure!(
        status.is_success(),
        "Parallax GraphQL failed: HTTP {status}: {body}"
    );
    ensure!(
        body.get("errors")
            .is_none_or(|errors| errors.as_array().is_none_or(Vec::is_empty)),
        "Parallax GraphQL returned errors: {body}"
    );
    Ok(body.get("data").cloned().unwrap_or(body))
}

async fn emit_issue_seed() -> anyhow::Result<()> {
    let mut encoded = Vec::new();
    let request: ExportTraceServiceRequest = shapes::issue_seed();
    request.encode(&mut encoded)?;
    let base = url_env("PARALLAX_OTLP_HTTP", "http://127.0.0.1:4318");
    let endpoint = if base.ends_with("/v1/traces") {
        base
    } else {
        format!("{base}/v1/traces")
    };
    let mut endpoints = vec![endpoint.clone()];
    if let Ok(mut fallback) = Url::parse(&endpoint)
        && fallback.port() == Some(4318)
    {
        fallback
            .set_port(Some(14318))
            .map_err(|_| anyhow::anyhow!("invalid OTLP fallback port"))?;
        let fallback = fallback.to_string();
        if fallback != endpoint {
            endpoints.push(fallback);
        }
    }

    let client = http_client()?;
    let mut failures = Vec::new();
    for endpoint in endpoints {
        match client
            .post(&endpoint)
            .header("content-type", "application/x-protobuf")
            .body(encoded.clone())
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => failures.push(format!("{endpoint}: HTTP {}", response.status())),
            Err(error) => failures.push(format!("{endpoint}: {error}")),
        }
    }
    bail!("issue seed OTLP request failed: {}", failures.join("; "))
}

async fn wait_for_issue() -> anyhow::Result<String> {
    let timeout = std::env::var("PARALLAX_TRACE_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(30);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    loop {
        let data = parallax_graphql("{ issues(limit: 8) { items { fingerprint title } } }")
            .await
            .unwrap_or_else(|_| json!({}));
        if let Some(fingerprint) = data
            .pointer("/issues/items/0/fingerprint")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            return Ok(fingerprint.to_owned());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Parallax produced no issue after the seed");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn assert_no_canary(label: &str, text: &str) -> anyhow::Result<()> {
    let lowered = text.to_ascii_lowercase();
    for needle in [CANARY_EMAIL, CANARY_TOKEN, CANARY_CARD, CANARY_JWT] {
        ensure!(
            !lowered.contains(&needle.to_ascii_lowercase()),
            "{label}: redaction canary leaked ({})",
            &needle[..needle.len().min(16)]
        );
    }
    Ok(())
}

const CANARY_EMAIL: &str = "alice@example.com";
const CANARY_TOKEN: &str = "sk-live-CANARY1234567890";
const CANARY_CARD: &str = "4111111111111111";
const CANARY_JWT: &str = "eyJhbGciOiJIUzI1NiJ9.CANARY.sig";

async fn checkout_request(
    quantity: u64,
    token: &str,
    request_prefix: &str,
    extra_headers: HeaderMap,
) -> anyhow::Result<Value> {
    let (status, body) = checkout_http(
        "WIDGET-1",
        quantity,
        token,
        request_prefix,
        extra_headers,
        &[],
    )
    .await?;
    ensure!(
        status.is_success(),
        "checkout failed: HTTP {status}: {body}"
    );
    println!("{body}");
    Ok(body)
}

async fn checkout_http(
    sku: &str,
    quantity: u64,
    token: &str,
    request_prefix: &str,
    extra_headers: HeaderMap,
    controls: &[(&str, Value)],
) -> anyhow::Result<(StatusCode, Value)> {
    let base = url_env("CHECKOUT_URL", "http://localhost:8088");
    let mut headers = json_headers();
    headers.extend(extra_headers);
    let mut request = json!({
        "tenant_id": "tenant-acme",
        "customer_id": "customer-acme-ava",
        "items": [{"sku": sku, "quantity": quantity}],
        "currency_code": "USD",
        "payment_method_token": token,
        "payment_method_type": "card",
        "request_id": format!("{request_prefix}-{}-{}", invocation::invocation_id(), uuid::Uuid::new_v4()),
    });
    for (key, value) in controls {
        request[key] = value.clone();
    }
    request_json(
        Method::POST,
        &format!("{base}/checkout"),
        headers,
        Some(request),
    )
    .await
}

async fn checkout_saga() -> anyhow::Result<i32> {
    for quantity in [1, 2] {
        let body =
            checkout_request(quantity, "tok_visa", "checkout-saga", HeaderMap::new()).await?;
        let order_id = body
            .get("order_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .context("checkout response omitted order_id")?;
        wait_for_fulfillment(order_id, "tenant-acme", true).await?;
    }
    println!("checkout saga reached fulfillment through the transactional outbox");
    Ok(0)
}

async fn checkout_outbox() -> anyhow::Result<i32> {
    let body = checkout_request(1, "tok_visa", "checkout-outbox", HeaderMap::new()).await?;
    let order_id = body
        .get("order_id")
        .and_then(Value::as_str)
        .context("checkout response omitted order_id")?;
    wait_for_fulfillment(order_id, "tenant-acme", true).await?;
    println!("checkout outbox order {order_id} reached fulfillment");
    Ok(0)
}

async fn wait_for_fulfillment(
    order: &str,
    tenant: &str,
    require_notification: bool,
) -> anyhow::Result<()> {
    let base = url_env("FULFILLMENT_URL", "http://localhost:8093");
    let token = std::env::var("FULFILLMENT_INTERNAL_TOKEN")
        .unwrap_or_else(|_| "fulfillment-internal:research-secret".into());
    let timeout = std::env::var("ASYNC_VERIFY_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(30);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    loop {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse()?);
        headers.insert("x-tenant-id", tenant.parse()?);
        let url = format!("{base}/verify/order?order={order}&tenant={tenant}");
        let (status, body) = request_json(Method::GET, &url, headers, None).await?;
        if status.is_success()
            && body.get("ready").and_then(Value::as_bool) == Some(true)
            && body.get("fulfillment_status").and_then(Value::as_str) == Some("completed")
            && (!require_notification
                || body
                    .get("notification_deliveries")
                    .and_then(Value::as_u64)
                    .is_some_and(|count| count >= 1))
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("fulfillment did not complete order {order} before timeout: {body}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn catalog_queries(label: &str, requests: u64) -> anyhow::Result<i32> {
    let base = url_env("CATALOG_URL", "http://localhost:8080");
    let query_text = if label == "exemplars" {
        "query Exemplars { products { items { id sku name } } }"
    } else {
        "query JvmMemoryPressure { products(tenantId: \"tenant-acme\", page: 0, size: 20, segment: \"standard\") { items { sku name priceMinor reviewsSlow { stars } } } }"
    };
    let query = json!({"query": query_text});
    for index in 1..=requests {
        let mut headers = json_headers();
        headers.insert("x-tenant-id", "tenant-acme".parse()?);
        let (status, body) = request_json(
            Method::POST,
            &format!("{base}/graphql"),
            headers,
            Some(query.clone()),
        )
        .await?;
        ensure!(
            status.is_success(),
            "{label} catalog query failed: HTTP {status}: {body}"
        );
        ensure!(
            body.get("errors")
                .is_none_or(|errors| errors.as_array().is_none_or(Vec::is_empty)),
            "{label} returned GraphQL errors: {body}"
        );
        ensure!(
            body.pointer("/data/products/items")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty()),
            "{label} returned no product data: {body}"
        );
        println!("catalog query {index}/{requests}");
    }
    println!("{label} catalog workload complete");
    Ok(0)
}

async fn jvm_memory_pressure() -> anyhow::Result<i32> {
    catalog_queries("jvm memory pressure baseline", 1).await?;
    catalog_queries("jvm memory pressure workload", positive_env("ROUNDS", 4)?).await
}

async fn graphql_request(query: &str) -> anyhow::Result<(StatusCode, Value)> {
    graphql_request_payload(query, None).await
}

async fn graphql_request_payload(
    query: &str,
    variables: Option<Value>,
) -> anyhow::Result<(StatusCode, Value)> {
    let base = url_env("CATALOG_URL", "http://localhost:8080");
    let mut headers = json_headers();
    headers.insert("x-tenant-id", "tenant-acme".parse()?);
    let body = match variables {
        Some(variables) => json!({"query": query, "variables": variables}),
        None => json!({"query": query}),
    };
    request_json(
        Method::POST,
        &format!("{base}/graphql"),
        headers,
        Some(body),
    )
    .await
}

fn ensure_graphql_success(label: &str, body: &Value) -> anyhow::Result<()> {
    ensure!(
        body.get("errors")
            .is_none_or(|errors| errors.as_array().is_none_or(Vec::is_empty)),
        "{label} returned GraphQL errors: {body}"
    );
    ensure!(
        body.pointer("/data/products/items")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty()),
        "{label} returned no product data: {body}"
    );
    Ok(())
}

async fn graphql_shapes() -> anyhow::Result<i32> {
    for (label, query) in [
        (
            "batched reviews",
            "query batchedReviews { products { items { id sku name reviews { text stars } } } }",
        ),
        (
            "slow reviews",
            "query slowReviews { products { items { id sku name reviewsSlow { text stars } } } }",
        ),
        (
            "risk score",
            "query normalRisk { products { items { id sku name riskScore } } }",
        ),
    ] {
        let (status, body) = graphql_request(query).await?;
        ensure!(status.is_success(), "{label} failed: HTTP {status}: {body}");
        ensure_graphql_success(label, &body)?;
        println!("[{label}] HTTP {status}\n{body}");
    }

    let synthetic_sku = std::env::var("CATALOG_SYNTHETIC_RISK_SCORE_FAILURE_SKU")
        .ok()
        .filter(|value| !value.trim().is_empty());
    if let Some(sku) = synthetic_sku {
        ensure!(
            sku.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
            "CATALOG_SYNTHETIC_RISK_SCORE_FAILURE_SKU contains unsupported characters"
        );
        let query =
            "query partialRisk($sku: String!) { product(sku: $sku) { id sku name riskScore } }";
        let (status, body) = graphql_request_payload(query, Some(json!({"sku": sku}))).await?;
        ensure!(
            status == StatusCode::OK,
            "partial riskScore failed: HTTP {status}: {body}"
        );
        ensure!(
            body.get("errors")
                .and_then(Value::as_array)
                .is_some_and(|errors| !errors.is_empty()),
            "partial riskScore omitted GraphQL errors: {body}"
        );
        ensure!(
            body.pointer("/data/product/sku").and_then(Value::as_str) == Some(sku.as_str()),
            "partial riskScore omitted the expected product: {body}"
        );
        println!("[partial riskScore] HTTP {status}\n{body}");
    } else {
        let (status, body) =
            graphql_request("query normalRisk { products { items { id sku name riskScore } } }")
                .await?;
        ensure!(
            status.is_success(),
            "normal riskScore failed: HTTP {status}: {body}"
        );
        ensure_graphql_success("normal riskScore", &body)?;
        println!("[normal riskScore] HTTP {status}\n{body}");
    }

    let operation = format!("lookup_{}", uuid::Uuid::new_v4().simple());
    let query = format!("query {operation} {{ products {{ items {{ id }} }} }}");
    let (status, body) = graphql_request(&query).await?;
    ensure!(
        status.is_success(),
        "high-cardinality operation failed: HTTP {status}: {body}"
    );
    ensure_graphql_success("high-cardinality operation", &body)?;
    println!("[high-cardinality operation {operation}] HTTP {status}");
    Ok(0)
}

async fn pricing_stream() -> anyhow::Result<i32> {
    let base = url_env("CHECKOUT_URL", "http://localhost:8088");
    let parallax_required = validate_parallax_mode()?;
    for (label, query, expected, validation) in [
        (
            "clean stream",
            "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=WIDGET-1&quantity=1",
            StatusCode::OK,
            "clean",
        ),
        (
            "unknown product",
            "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=NO-SUCH-SKU&quantity=1",
            StatusCode::NOT_FOUND,
            "unknown",
        ),
        (
            "client cancellation",
            "?tenant_id=tenant-acme&customer_id=customer-acme-ava&sku=WIDGET-1&quantity=1&delay_ms=1",
            StatusCode::OK,
            "cancelled",
        ),
    ] {
        let trace_id = hex_id(16);
        let traceparent = format!("00-{trace_id}-{}-01", hex_id(8));
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", traceparent.parse()?);
        let (status, body) = request_json(
            Method::GET,
            &format!("{base}/quote-stream{query}"),
            headers,
            None,
        )
        .await?;
        ensure!(
            status == expected,
            "{label}: expected HTTP {}, got {status}: {body}",
            expected.as_u16()
        );
        validate_pricing_stream_response(label, &body, validation)?;
        println!("{label}: HTTP {status} traceparent={traceparent} {body}");
        if parallax_required {
            wait_for_pricing_trace(&trace_id, label, validation).await?;
        }
    }
    if parallax_required {
        println!("A7b Parallax trace assertions PASS");
    } else {
        println!("A7b Parallax trace assertions SKIPPED by SCENARIO_PARALLAX_MODE=skip");
    }
    Ok(0)
}

fn validate_parallax_mode() -> anyhow::Result<bool> {
    match std::env::var("SCENARIO_PARALLAX_MODE")
        .unwrap_or_else(|_| "required".to_owned())
        .as_str()
    {
        "required" => Ok(true),
        "skip" => Ok(false),
        value => bail!("SCENARIO_PARALLAX_MODE must be required or skip, got {value}"),
    }
}

fn validate_pricing_stream_response(
    label: &str,
    body: &Value,
    validation: &str,
) -> anyhow::Result<()> {
    match validation {
        "clean" | "cancelled" => {
            ensure!(
                body.get("error").is_none_or(Value::is_null),
                "{label}: response contained an error: {body}"
            );
            ensure!(
                body.get("streamed_quotes").and_then(Value::as_u64) == Some(1),
                "{label}: response did not prove one delivered quote message: {body}"
            );
            let expected_cancelled = validation == "cancelled";
            ensure!(
                body.get("cancelled").and_then(Value::as_bool) == Some(expected_cancelled),
                "{label}: response cancellation flag was not {expected_cancelled}: {body}"
            );
        }
        "unknown" => {
            ensure!(
                body.get("error").and_then(Value::as_str) == Some("product_not_found"),
                "{label}: response did not prove the pricing stream rejection: {body}"
            );
            ensure!(
                body.get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.trim().is_empty()),
                "{label}: pricing stream rejection omitted a message: {body}"
            );
        }
        other => bail!("{label}: unknown pricing stream validation {other}"),
    }
    Ok(())
}

fn pricing_trace_matches(data: &Value, expected_message_type: &str) -> bool {
    let Some(spans) = data.pointer("/trace/spans").and_then(Value::as_array) else {
        return false;
    };
    let checkout_seen = spans.iter().any(|span| {
        span.get("service").and_then(Value::as_str) == Some("checkout")
            && span.get("name").and_then(Value::as_str) == Some("checkout.quote_stream")
    });
    let pricing_seen = spans.iter().any(|span| {
        if span.get("service").and_then(Value::as_str) != Some("pricing")
            || span.get("name").and_then(Value::as_str) != Some("pricing.quote_stream.send")
        {
            return false;
        }
        let attributes = span.get("attributes").and_then(|attributes| {
            if let Some(raw) = attributes.as_str() {
                serde_json::from_str::<Value>(raw).ok()
            } else {
                Some(attributes.clone())
            }
        });
        attributes
            .as_ref()
            .and_then(|attributes| attributes.get("rpc.message.type"))
            .and_then(Value::as_str)
            == Some(expected_message_type)
    });
    checkout_seen && pricing_seen
}

async fn wait_for_pricing_trace(
    trace_id: &str,
    label: &str,
    validation: &str,
) -> anyhow::Result<()> {
    let timeout = positive_env("PARALLAX_TRACE_TIMEOUT_SECONDS", 30)?;
    let poll = positive_env("PARALLAX_TRACE_POLL_SECONDS", 1)?;
    let expected_message_type = if validation == "unknown" {
        "ERROR"
    } else {
        "SENT"
    };
    let query =
        format!("{{ trace(traceId: {trace_id:?}) {{ spans {{ service name attributes }} }} }}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    let mut last = Value::Null;
    loop {
        if let Ok(data) = parallax_graphql(&query).await {
            last = data;
            if pricing_trace_matches(&last, expected_message_type) {
                println!("Parallax trace PASS: {label}");
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Parallax trace FAIL: {label} (trace={trace_id}); last response: {last}");
        }
        tokio::time::sleep(Duration::from_secs(poll)).await;
    }
}

fn hex_id(bytes: usize) -> String {
    let id = uuid::Uuid::new_v4().as_u128();
    format!("{id:032x}")[..bytes * 2].to_owned()
}

async fn seeded_order_replay() -> anyhow::Result<i32> {
    let base = url_env("FULFILLMENT_URL", "http://localhost:8093");
    let token = std::env::var("FULFILLMENT_INTERNAL_TOKEN")
        .unwrap_or_else(|_| "fulfillment-internal:research-secret".into());
    for (order, tenant) in [
        ("order-acme-1001", "tenant-acme"),
        ("order-nova-2001", "tenant-nova"),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {token}").parse()?);
        headers.insert("x-tenant-id", tenant.parse()?);
        let url = format!("{base}/publish?order={order}&tenant={tenant}");
        let (status, body) = request_json(Method::POST, &url, headers, None).await?;
        ensure!(
            status.is_success(),
            "seeded order {order} publish failed: {status}: {body}"
        );
        wait_for_fulfillment(order, tenant, false).await?;
    }
    println!("seeded-order replay reached fulfillment and notifications");
    Ok(0)
}

async fn baggage_checkout() -> anyhow::Result<i32> {
    let tenant = nonempty_env("A10_TENANT").unwrap_or_else(|| "tenant-acme".to_owned());
    let customer = nonempty_env("A10_CUSTOMER").unwrap_or_else(|| "customer-acme-ava".to_owned());
    let tier = nonempty_env("A10_TIER").unwrap_or_else(|| "pro".to_owned());
    let mut headers = HeaderMap::new();
    headers.insert(
        "baggage",
        format!("tenant.id={tenant},user.tier={tier}").parse()?,
    );
    let controls = [
        ("tenant_id", json!(tenant)),
        ("customer_id", json!(customer)),
        ("tier", json!(tier)),
    ];
    let (status, body) =
        checkout_http("WIDGET-1", 1, "tok_visa", "baggage", headers, &controls).await?;
    ensure!(
        status.is_success(),
        "A10 baggage checkout failed: HTTP {status}: {body}"
    );
    println!("W3C baggage checkout emitted tenant={tenant} tier={tier}");
    Ok(0)
}

async fn checkout_probe(label: &str) -> anyhow::Result<i32> {
    let base = url_env("CHECKOUT_BASE", "http://localhost:8088");
    let request = json!({
        "tenant_id": "tenant-acme",
        "customer_id": "customer-acme-ava",
        "items": [{"sku": "WIDGET-1", "quantity": 1}],
        "currency_code": "USD",
        "payment_method_token": "tok_visa",
        "payment_method_type": "card",
        "request_id": format!("probe-{}-{}", invocation::invocation_id(), uuid::Uuid::new_v4()),
    });
    let (status, body) = request_json(
        Method::POST,
        &format!("{base}/checkout"),
        json_headers(),
        Some(request),
    )
    .await?;
    ensure!(
        status.is_success(),
        "{label}: checkout probe failed: HTTP {status}: {body}"
    );
    println!(
        "{label}: {}",
        body.get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    );
    let settle = positive_env("B23_SETTLE_SECONDS", 1)?;
    tokio::time::sleep(Duration::from_secs(settle)).await;
    Ok(0)
}

async fn checkout_expected(
    label: &str,
    sku: &str,
    token: &str,
    request_prefix: &str,
    controls: &[(&str, Value)],
    expected: StatusCode,
) -> anyhow::Result<Value> {
    let (status, body) =
        checkout_http(sku, 1, token, request_prefix, HeaderMap::new(), controls).await?;
    ensure!(
        status == expected,
        "{label}: expected HTTP {expected}, got {status}: {body}"
    );
    println!("{label}: HTTP {status} {body}");
    Ok(body)
}

async fn delayed_checkout_burst(label: &str, legacy_prefix: &str) -> anyhow::Result<i32> {
    let requests_name = if legacy_prefix == "B5" {
        "B5_REQUESTS"
    } else {
        "REQUESTS"
    };
    let delay_name = if legacy_prefix == "B5" {
        "B5_DELAY_MS"
    } else {
        "DELAY_MS"
    };
    let requests = positive_env(requests_name, 12)?;
    let delay_ms = positive_env(delay_name, 300)?;
    for index in 1..=requests {
        let controls = [("delay_ms", json!(delay_ms))];
        checkout_expected(
            &format!("{label} request {index}/{requests}"),
            "WIDGET-1",
            "tok_visa",
            &format!("{legacy_prefix}-delay-{index}"),
            &controls,
            StatusCode::OK,
        )
        .await?;
    }
    println!("{label}: {requests} delayed checkout requests completed");
    Ok(0)
}

async fn checkout_pressure() -> anyhow::Result<i32> {
    let delay_ms = positive_env("DELAY_MS", 8_000)?;
    let concurrency = positive_env("CONCURRENCY", 12)?;
    checkout_expected(
        "runtime saturation baseline",
        "WIDGET-1",
        "tok_visa",
        "a22-baseline",
        &[],
        StatusCode::OK,
    )
    .await?;

    let mut requests = JoinSet::new();
    requests.spawn(async move {
        let controls = [("delay_ms", json!(delay_ms))];
        checkout_http(
            "WIDGET-1",
            1,
            "tok_visa",
            "a22-delay",
            HeaderMap::new(),
            &controls,
        )
        .await
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    for index in 1..=concurrency {
        requests.spawn(async move {
            checkout_http(
                "WIDGET-1",
                1,
                "tok_visa",
                &format!("a22-concurrent-{index}"),
                HeaderMap::new(),
                &[],
            )
            .await
        });
    }
    while let Some(result) = requests.join_next().await {
        let (status, body) = result??;
        ensure!(
            status.is_success(),
            "runtime saturation checkout failed: HTTP {status}: {body}"
        );
    }
    println!(
        "runtime saturation completed: one {delay_ms}ms delayed request plus {concurrency} concurrent checkouts"
    );
    Ok(0)
}

async fn delayed_checkout_concurrency(label: &str) -> anyhow::Result<i32> {
    let requests = positive_env("B10_REQUESTS", 12)?;
    let mut children = JoinSet::new();
    for index in 1..=requests {
        children.spawn(async move {
            let controls = [("delay_ms", json!(150_u64))];
            checkout_http(
                "WIDGET-1",
                1,
                "tok_visa",
                &format!("b10-{index}"),
                HeaderMap::new(),
                &controls,
            )
            .await
        });
    }
    while let Some(result) = children.join_next().await {
        let (status, body) = result??;
        ensure!(
            status.is_success(),
            "{label} request failed: HTTP {status}: {body}"
        );
    }
    println!("{label}: {requests} concurrent delayed checkout requests completed");
    Ok(0)
}

async fn request_metric_shapes() -> anyhow::Result<i32> {
    for (index, sku) in ["WIDGET-1", "WIDGET-2", "GADGET-1", "WIDGET-1"]
        .into_iter()
        .enumerate()
    {
        checkout_expected(
            &format!("request metrics {sku}"),
            sku,
            "tok_visa",
            &format!("a30-{sku}-{index}"),
            &[],
            StatusCode::OK,
        )
        .await?;
    }
    println!("request metric shape workload completed");
    Ok(0)
}

async fn payment_failure_latency() -> anyhow::Result<i32> {
    checkout_expected(
        "handled payment decline",
        "WIDGET-1",
        "tok_decline",
        "payment-decline",
        &[],
        StatusCode::PAYMENT_REQUIRED,
    )
    .await?;
    let controls = [("slow", json!(500_u64))];
    checkout_expected(
        "payment latency",
        "WIDGET-1",
        "tok_visa",
        "payment-latency",
        &controls,
        StatusCode::OK,
    )
    .await?;
    Ok(0)
}

async fn checkout_chaos() -> anyhow::Result<i32> {
    let controls = [
        ("retry", json!(2_u32)),
        ("timeout_ms", json!(50_u64)),
        ("delay_ms", json!(350_u64)),
    ];
    checkout_expected(
        "checkout retry/deadline",
        "WIDGET-1",
        "tok_visa",
        "checkout-chaos-deadline",
        &controls,
        StatusCode::BAD_GATEWAY,
    )
    .await?;
    let controls = [("delay_ms", json!(400_u64))];
    checkout_expected(
        "checkout delayed dependency",
        "WIDGET-1",
        "tok_visa",
        "checkout-chaos-delay",
        &controls,
        StatusCode::OK,
    )
    .await?;
    Ok(0)
}

async fn grpc_deadline_retry() -> anyhow::Result<i32> {
    let controls = [
        ("retry", json!(2_u32)),
        ("timeout_ms", json!(100_u64)),
        ("delay_ms", json!(350_u64)),
    ];
    checkout_expected(
        "gRPC pricing deadline",
        "WIDGET-1",
        "tok_visa",
        "grpc-deadline",
        &controls,
        StatusCode::BAD_GATEWAY,
    )
    .await?;
    Ok(0)
}

async fn provider_degradation() -> anyhow::Result<i32> {
    let controls = [("degrade", json!(true))];
    let body = checkout_expected(
        "provider unavailable degradation",
        "WIDGET-1",
        "tok_unavailable",
        "provider-degraded",
        &controls,
        StatusCode::OK,
    )
    .await?;
    ensure!(
        matches!(
            body.get("status").and_then(Value::as_str),
            Some("degraded" | "payment_pending")
        ),
        "provider degradation response was not typed: {body}"
    );
    let controls = [("delay_ms", json!(500_u64))];
    checkout_expected(
        "provider delayed dependency",
        "WIDGET-1",
        "tok_visa",
        "provider-delay",
        &controls,
        StatusCode::OK,
    )
    .await?;
    Ok(0)
}

async fn handled_unhandled() -> anyhow::Result<i32> {
    checkout_expected(
        "handled provider decline",
        "WIDGET-1",
        "tok_decline",
        "handled-decline",
        &[],
        StatusCode::PAYMENT_REQUIRED,
    )
    .await?;
    checkout_expected(
        "provider internal error",
        "WIDGET-1",
        "tok_internal",
        "handled-internal",
        &[],
        StatusCode::BAD_GATEWAY,
    )
    .await?;
    println!("handled 402 and provider-internal 502 outcomes verified");
    Ok(0)
}

fn set_flag_variant(path: &Path, flag_key: &str, variant: &str) -> anyhow::Result<()> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut document: Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let flag = document
        .pointer_mut(&format!("/flags/{flag_key}"))
        .with_context(|| format!("flagd file has no {flag_key} flag"))?;
    flag["defaultVariant"] = json!(variant);
    let encoded = serde_json::to_vec_pretty(&document)?;
    fs::write(path, format!("{}\n", String::from_utf8(encoded)?))
        .with_context(|| format!("failed to update {}", path.display()))?;
    Ok(())
}

async fn with_flag_variant<F, Fut, T>(
    root: &Path,
    flag_key: &str,
    initial_variant: &str,
    operation: F,
) -> anyhow::Result<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let path = root.join("flags/flagd.json");
    let backup = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    set_flag_variant(&path, flag_key, initial_variant)?;
    let result = operation().await;
    let restore =
        fs::write(&path, backup).with_context(|| format!("failed to restore {}", path.display()));
    match (result, restore) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(restore_error)) => bail!(
            "flagd scenario failed: {error}; restoring {} failed: {restore_error}",
            path.display()
        ),
    }
}

async fn feature_flag_variants() -> anyhow::Result<i32> {
    let root = repository_root();
    let compose_file = root.join("deploy/docker-compose.yml");
    run_compose(
        &root,
        &compose_file,
        None,
        "v1",
        &[
            "up",
            "-d",
            "flagd",
            "pricing",
            "inventory",
            "recommendation",
            "checkout",
        ],
    )
    .await?;
    wait_for_checkout(&url_env("CHECKOUT_URL", "http://localhost:8088")).await?;
    let settle = positive_env("A14_FLAG_SETTLE_SECONDS", 12)?;
    let requests = positive_env("A14_REQUESTS", 5)?;
    let flag_root = root.clone();
    with_flag_variant(&root, "checkoutFlow", "control", || async move {
        let path = flag_root.join("flags/flagd.json");
        for variant in ["control", "orchestrated", "control"] {
            set_flag_variant(&path, "checkoutFlow", variant)?;
            tokio::time::sleep(Duration::from_secs(settle)).await;
            for index in 1..=requests {
                checkout_variant_request(
                    variant,
                    &format!("flag-variant-{variant}-{index}"),
                    false,
                )
                .await?;
            }
        }
        println!("checkoutFlow control → orchestrated → control completed without restart");
        Ok(0)
    })
    .await
}

async fn feature_flag_topology_compare() -> anyhow::Result<i32> {
    let root = repository_root();
    let compose_file = root.join("deploy/docker-compose.yml");
    run_compose(
        &root,
        &compose_file,
        None,
        "v1",
        &[
            "up",
            "-d",
            "flagd",
            "pricing",
            "inventory",
            "recommendation",
            "checkout",
        ],
    )
    .await?;
    wait_for_checkout(&url_env("CHECKOUT_URL", "http://localhost:8088")).await?;
    let settle = positive_env("A20_FLAG_SETTLE_SECONDS", 12)?;
    let flag_root = root.clone();
    with_flag_variant(&root, "checkoutFlow", "control", || async move {
        let path = flag_root.join("flags/flagd.json");
        set_flag_variant(&path, "checkoutFlow", "control")?;
        tokio::time::sleep(Duration::from_secs(settle)).await;
        let control = checkout_variant_request("control", "flag-compare-control", true).await?;
        set_flag_variant(&path, "checkoutFlow", "orchestrated")?;
        tokio::time::sleep(Duration::from_secs(settle)).await;
        let orchestrated =
            checkout_variant_request("orchestrated", "flag-compare-orchestrated", true).await?;
        ensure!(
            control.get("recommendation").is_some_and(Value::is_null),
            "control checkout unexpectedly included recommendation topology: {control}"
        );
        ensure!(
            orchestrated
                .get("recommendation")
                .is_some_and(|value| !value.is_null()),
            "orchestrated checkout omitted recommendation topology: {orchestrated}"
        );
        println!("checkoutFlow topology comparison passed");
        Ok(0)
    })
    .await
}

async fn checkout_variant_request(
    variant: &str,
    request_prefix: &str,
    compare: bool,
) -> anyhow::Result<Value> {
    let body = checkout_expected(
        &format!("checkoutFlow={variant}"),
        "WIDGET-1",
        "tok_visa",
        request_prefix,
        &[],
        StatusCode::OK,
    )
    .await?;
    ensure!(
        body.get("feature_variant").and_then(Value::as_str) == Some(variant),
        "checkout response reported the wrong feature variant: expected {variant}: {body}"
    );
    if compare {
        println!(
            "checkoutFlow={variant}: recommendation={}",
            body["recommendation"]
        );
    }
    Ok(body)
}

async fn storefront_graphql(kind: &str) -> anyhow::Result<i32> {
    let base = url_env("STOREFRONT_URL", "http://localhost:8094");
    let parallax_required = kind == "pricing" && validate_parallax_mode()?;
    let trace_id = hex_id(16);
    let (operation_name, query, variables) = if kind == "pricing" {
        (
            "StorefrontQuote",
            "query StorefrontQuote($input: QuoteInput!) { quote(input: $input) { quoteId status lines { sku quantity unitPrice { currencyCode amountMinor } lineTotal { currencyCode amountMinor } } subtotal { currencyCode amountMinor } discountTotal { currencyCode amountMinor } taxTotal { currencyCode amountMinor } grandTotal { currencyCode amountMinor } validForSeconds pricingVersion } }",
            Some(
                json!({"input": {"tenantId": "tenant-acme", "customerId": "customer-acme-ava", "currencyCode": "USD", "items": [{"sku": "WIDGET-1", "quantity": 2}]}}),
            ),
        )
    } else {
        (
            "StorefrontCatalog",
            "query StorefrontCatalog { products(tenantId: \"tenant-acme\", page: 0, size: 10, segment: \"standard\") { items { sku name priceMinor category { slug name } variants { sku name price { amountMinor currency } } reviews { stars title } } page size totalElements totalPages hasNext experience } categories(tenantId: \"tenant-acme\") { slug name }",
            None,
        )
    };
    let mut headers = json_headers();
    headers.insert(
        "traceparent",
        format!("00-{trace_id}-{}-01", hex_id(8)).parse()?,
    );
    let mut request = json!({"operationName": operation_name, "query": query});
    if let Some(variables) = variables {
        request["variables"] = variables;
    }
    let (status, body) = request_json(
        Method::POST,
        &format!("{base}/graphql"),
        headers,
        Some(request),
    )
    .await?;
    ensure!(
        status.is_success(),
        "storefront {kind} request failed: {status}: {body}"
    );
    ensure!(
        body.get("errors")
            .is_none_or(|errors| errors.as_array().is_none_or(Vec::is_empty)),
        "storefront {kind} returned GraphQL errors: {body}"
    );
    if kind == "pricing" {
        ensure!(
            body.pointer("/data/quote/quoteId")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
                && body.pointer("/data/quote/status").and_then(Value::as_str)
                    == Some("QUOTE_STATUS_READY")
                && body
                    .pointer("/data/quote/lines")
                    .and_then(Value::as_array)
                    .is_some_and(|lines| lines.len() == 1)
                && body
                    .pointer("/data/quote/grandTotal/currencyCode")
                    .and_then(Value::as_str)
                    == Some("USD"),
            "storefront pricing response did not prove a ready quote: {body}"
        );
    } else {
        ensure!(
            body.pointer("/data/products/items")
                .and_then(Value::as_array)
                .is_some_and(|items| !items.is_empty())
                && body
                    .pointer("/data/categories")
                    .and_then(Value::as_array)
                    .is_some_and(|categories| !categories.is_empty()),
            "storefront catalog response did not prove products and categories: {body}"
        );
    }
    println!("storefront {kind}: {body}");
    if parallax_required {
        wait_for_storefront_pricing_trace(&trace_id).await?;
    }
    Ok(0)
}

async fn wait_for_storefront_pricing_trace(trace_id: &str) -> anyhow::Result<()> {
    let timeout = positive_env("PARALLAX_TRACE_TIMEOUT_SECONDS", 30)?;
    let poll = positive_env("PARALLAX_TRACE_POLL_SECONDS", 1)?;
    let query = format!("{{ trace(traceId: {trace_id:?}) {{ spans {{ service name }} }} }}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    let mut last = Value::Null;
    loop {
        if let Ok(data) = parallax_graphql(&query).await {
            last = data;
            if let Some(spans) = last.pointer("/trace/spans").and_then(Value::as_array) {
                let storefront_seen = spans
                    .iter()
                    .any(|span| span.get("service").and_then(Value::as_str) == Some("storefront"));
                let pricing_seen = spans.iter().any(|span| {
                    span.get("service").and_then(Value::as_str) == Some("pricing")
                        && span.get("name").and_then(Value::as_str) == Some("pricing.quote")
                });
                if storefront_seen && pricing_seen {
                    println!("Parallax trace PASS: storefront pricing {trace_id}");
                    return Ok(());
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "Parallax trace FAIL: storefront pricing (trace={trace_id}); last response: {last}"
            );
        }
        tokio::time::sleep(Duration::from_secs(poll)).await;
    }
}

async fn inventory_pressure() -> anyhow::Result<i32> {
    let base = url_env("INVENTORY_URL", "http://localhost:8089");
    let tenant = nonempty_env("INVENTORY_TENANT_ID").unwrap_or_else(|| "tenant-acme".to_owned());
    let configured_request = env_or_fallback("A25_CHECKOUT_REQUEST_ID", "CHECKOUT_REQUEST_ID");
    let configured_lease = env_or_fallback("A25_CHECKOUT_LEASE_TOKEN", "CHECKOUT_LEASE_TOKEN");
    let (request_id, lease_token) = match (configured_request, configured_lease) {
        (Some(request_id), Some(lease_token)) => (request_id, lease_token),
        (None, None) => create_checkout_fence(&tenant).await?,
        _ => bail!(
            "set both A25_CHECKOUT_REQUEST_ID and A25_CHECKOUT_LEASE_TOKEN, or unset both for automatic fence creation"
        ),
    };
    let run_id =
        nonempty_env("A25_RUN_ID").unwrap_or_else(|| format!("a25-{}", uuid::Uuid::new_v4()));
    let parallax_required = validate_parallax_mode()?;

    for (label, sku, extra) in [
        ("normal", "WIDGET-1", ""),
        ("pg_sleep 400ms", "WIDGET-2", "&slow=400"),
        ("db_n1=12", "GADGET-1", "&db_n1=12"),
    ] {
        let reservation_id = format!("{run_id}-{label}").replace(' ', "-");
        let trace_id = hex_id(16);
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            format!("00-{trace_id}-{}-01", hex_id(8)).parse()?,
        );
        let mut query = vec![
            ("tenant_id".to_owned(), tenant.clone()),
            ("reservation_id".to_owned(), reservation_id.clone()),
            ("sku".to_owned(), sku.to_owned()),
            ("quantity".to_owned(), "1".to_owned()),
            ("checkout_request_id".to_owned(), request_id.clone()),
            ("checkout_lease_token".to_owned(), lease_token.clone()),
        ];
        match extra {
            "&slow=400" => query.push(("slow".to_owned(), "400".to_owned())),
            "&db_n1=12" => query.push(("db_n1".to_owned(), "12".to_owned())),
            other => ensure!(other.is_empty(), "unsupported A25 query controls: {other}"),
        }
        let url = url_with_query(&base, "/reserve", &query)?;
        let (status, body) = request_json(Method::GET, &url, headers, None).await?;
        ensure!(
            status == StatusCode::OK,
            "{label}: expected HTTP 200, got {status}: {body}"
        );
        ensure!(
            body.get("tenant_id").and_then(Value::as_str) == Some(tenant.as_str())
                && body.get("reservation_id").and_then(Value::as_str)
                    == Some(reservation_id.as_str())
                && body.get("sku").and_then(Value::as_str) == Some(sku)
                && body.get("reserved").and_then(Value::as_u64) == Some(1)
                && body
                    .get("location_id")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty())
                && body.get("status").and_then(Value::as_str) == Some("reserved"),
            "{label}: response did not prove a real reservation: {body}"
        );
        let location_id = body
            .get("location_id")
            .and_then(Value::as_str)
            .context("inventory reservation omitted location_id")?;
        release_inventory_reservation(
            &base,
            &tenant,
            &reservation_id,
            sku,
            location_id,
            &request_id,
            &lease_token,
        )
        .await?;
        if parallax_required {
            wait_for_inventory_trace(&trace_id, label, true).await?;
        }
        println!("A25 {label} reservation/release verified");
    }

    let pool_start_delay = match nonempty_env("A25_POOL_START_DELAY_SECONDS") {
        Some(raw) => {
            let seconds = raw
                .parse::<f64>()
                .with_context(|| format!("A25_POOL_START_DELAY_SECONDS must be positive: {raw}"))?;
            ensure!(
                seconds.is_finite() && seconds > 0.0,
                "A25_POOL_START_DELAY_SECONDS must be positive: {raw}"
            );
            Duration::from_secs_f64(seconds)
        }
        None => Duration::from_millis(positive_env("A25_POOL_START_DELAY_MS", 500)?),
    };
    let mut holds = JoinSet::new();
    for index in 1..=10 {
        let reservation_id = format!("{run_id}-hold-{index}");
        let trace_id = hex_id(16);
        let url = url_with_query(
            &base,
            "/reserve",
            &[
                ("tenant_id".to_owned(), tenant.clone()),
                ("reservation_id".to_owned(), reservation_id.clone()),
                ("sku".to_owned(), "WIDGET-2".to_owned()),
                ("quantity".to_owned(), "1".to_owned()),
                ("checkout_request_id".to_owned(), request_id.clone()),
                ("checkout_lease_token".to_owned(), lease_token.clone()),
                ("hold_ms".to_owned(), "4000".to_owned()),
            ],
        )?;
        holds.spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                "traceparent",
                format!("00-{trace_id}-{}-01", hex_id(8)).parse()?,
            );
            let (status, body) = request_json(Method::GET, &url, headers, None).await?;
            Ok::<_, anyhow::Error>((index, reservation_id, trace_id, status, body))
        });
    }
    tokio::time::sleep(pool_start_delay).await;

    let pool_trace_id = hex_id(16);
    let mut pool_headers = HeaderMap::new();
    pool_headers.insert(
        "traceparent",
        format!("00-{pool_trace_id}-{}-01", hex_id(8)).parse()?,
    );
    let pool_reservation = format!("{run_id}-pool-probe");
    let pool_url = url_with_query(
        &base,
        "/reserve",
        &[
            ("tenant_id".to_owned(), tenant.clone()),
            ("reservation_id".to_owned(), pool_reservation.clone()),
            ("sku".to_owned(), "WIDGET-1".to_owned()),
            ("quantity".to_owned(), "1".to_owned()),
            ("checkout_request_id".to_owned(), request_id.clone()),
            ("checkout_lease_token".to_owned(), lease_token.clone()),
        ],
    )?;
    let pool_result = request_json(Method::GET, &pool_url, pool_headers, None).await;
    let mut hold_error = None;
    let mut hold_traces = Vec::new();
    while let Some(result) = holds.join_next().await {
        match result {
            Ok(Ok((index, reservation_id, trace_id, status, body))) => {
                hold_traces.push((trace_id, format!("hold-{index}")));
                let validation = match status {
                    StatusCode::OK => {
                        let location_id = body
                            .get("location_id")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty());
                        if let Some(location_id) = location_id {
                            release_inventory_reservation(
                                &base,
                                &tenant,
                                &reservation_id,
                                "WIDGET-2",
                                location_id,
                                &request_id,
                                &lease_token,
                            )
                            .await
                        } else {
                            Err(anyhow::anyhow!("pool hold omitted a non-empty location_id"))
                        }
                    }
                    StatusCode::SERVICE_UNAVAILABLE => {
                        if body
                            .get("error")
                            .and_then(Value::as_str)
                            .is_some_and(|error| {
                                error == "inventory_unavailable" || error == "reservation_rejected"
                            })
                        {
                            Ok(())
                        } else {
                            Err(anyhow::anyhow!(
                                "hold-{index}: unexpected 503 response: {body}"
                            ))
                        }
                    }
                    other => Err(anyhow::anyhow!(
                        "hold-{index}: expected HTTP 200 or 503, got {other}: {body}"
                    )),
                };
                if let Err(error) = validation
                    && hold_error.is_none()
                {
                    hold_error = Some(error);
                }
            }
            Ok(Err(error)) => {
                if hold_error.is_none() {
                    hold_error = Some(error);
                }
            }
            Err(error) => {
                if hold_error.is_none() {
                    hold_error = Some(anyhow::anyhow!("pool hold task failed: {error}"));
                }
            }
        }
    }
    if let Some(error) = hold_error {
        return Err(error);
    }
    let (pool_status, pool_body) = pool_result?;
    ensure!(
        pool_status == StatusCode::SERVICE_UNAVAILABLE,
        "pool pressure expected HTTP 503, got {pool_status}: {pool_body}"
    );
    ensure!(
        pool_body.get("error").and_then(Value::as_str) == Some("inventory_unavailable")
            && pool_body.get("reservation_id").and_then(Value::as_str)
                == Some(pool_reservation.as_str()),
        "pool pressure response did not prove inventory_unavailable: {pool_body}"
    );
    if parallax_required {
        for (trace_id, label) in hold_traces {
            wait_for_inventory_trace(&trace_id, &label, false).await?;
        }
    }
    if parallax_required {
        wait_for_inventory_trace(&pool_trace_id, "pool pressure", false).await?;
    }
    println!("A25 PostgreSQL reservation, slow query, N+1, and pool assertions passed");
    Ok(0)
}

async fn create_checkout_fence(tenant: &str) -> anyhow::Result<(String, String)> {
    ensure!(
        tenant == "tenant-acme",
        "automatic A25 checkout-fence creation supports tenant-acme only; set explicit A25 fence variables for {tenant}"
    );
    let request_id = format!("a25-fence-{}", uuid::Uuid::new_v4());
    let controls = [("request_id", json!(request_id.clone()))];
    let (status, body) = checkout_http(
        "WIDGET-1",
        1,
        "tok_visa",
        "a25-fence",
        HeaderMap::new(),
        &controls,
    )
    .await?;
    ensure!(
        status.is_success(),
        "automatic A25 fence checkout failed: HTTP {status}: {body}"
    );

    let root = repository_root();
    let compose = root.join("deploy/docker-compose.yml");
    let sql = format!(
        "SELECT lease_token FROM checkout_attempts WHERE tenant_id = 'tenant-acme' AND request_id = '{}' ORDER BY created_at DESC LIMIT 1",
        request_id.replace('\'', "''")
    );
    let args = vec![
        "compose".to_owned(),
        "-f".to_owned(),
        compose.display().to_string(),
        "exec".to_owned(),
        "-T".to_owned(),
        "postgres".to_owned(),
        "psql".to_owned(),
        "-U".to_owned(),
        "postgres".to_owned(),
        "-d".to_owned(),
        "playground".to_owned(),
        "-At".to_owned(),
        "-c".to_owned(),
        sql,
    ];
    let (code, output) = capture_program(PathBuf::from("docker"), args, Some(root)).await?;
    ensure!(code == 0, "automatic A25 fence lookup failed: {output}");
    let lease_token = output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| uuid::Uuid::parse_str(line).is_ok())
        .context("automatic A25 fence lookup returned no lease token")?
        .to_owned();
    println!("A25 created checkout fence {request_id}");
    Ok((request_id, lease_token))
}

async fn release_inventory_reservation(
    base: &str,
    tenant: &str,
    reservation_id: &str,
    sku: &str,
    location_id: &str,
    request_id: &str,
    lease_token: &str,
) -> anyhow::Result<()> {
    let mut headers = json_headers();
    let trace_id = hex_id(16);
    headers.insert(
        "traceparent",
        format!("00-{trace_id}-{}-01", hex_id(8)).parse()?,
    );
    let payload = json!({
        "tenant_id": tenant,
        "reservation_id": reservation_id,
        "sku": sku,
        "quantity": 1,
        "location_id": location_id,
        "checkout_request_id": request_id,
        "checkout_lease_token": lease_token,
    });
    let (status, body) = request_json(
        Method::POST,
        &format!("{base}/release"),
        headers,
        Some(payload),
    )
    .await?;
    ensure!(
        status == StatusCode::OK
            && body.get("status").and_then(Value::as_str) == Some("released")
            && body.get("reservation_id").and_then(Value::as_str) == Some(reservation_id)
            && body.get("sku").and_then(Value::as_str) == Some(sku)
            && body.get("released").and_then(Value::as_u64) == Some(1),
        "inventory release failed for {reservation_id}: HTTP {status}: {body}"
    );
    Ok(())
}

fn inventory_trace_matches(data: &Value, require_postgres: bool) -> bool {
    let Some(spans) = data.pointer("/trace/spans").and_then(Value::as_array) else {
        return false;
    };
    let inventory_seen = spans.iter().any(|span| {
        span.get("service").and_then(Value::as_str) == Some("inventory")
            && span.get("name").and_then(Value::as_str) == Some("inventory.reserve")
    });
    if !require_postgres {
        return inventory_seen;
    }
    let postgres_seen = spans.iter().any(|span| {
        if span.get("service").and_then(Value::as_str) != Some("inventory")
            || span.get("name").and_then(Value::as_str) != Some("postgres.query")
        {
            return false;
        }
        let attributes = span.get("attributes").and_then(|attributes| {
            if let Some(raw) = attributes.as_str() {
                serde_json::from_str::<Value>(raw).ok()
            } else {
                Some(attributes.clone())
            }
        });
        attributes
            .as_ref()
            .and_then(|attributes| attributes.get("db.system.name"))
            .and_then(Value::as_str)
            == Some("postgresql")
    });
    inventory_seen && postgres_seen
}

async fn wait_for_inventory_trace(
    trace_id: &str,
    label: &str,
    require_postgres: bool,
) -> anyhow::Result<()> {
    let timeout = positive_env("PARALLAX_TRACE_TIMEOUT_SECONDS", 30)?;
    let poll = positive_env("PARALLAX_TRACE_POLL_SECONDS", 1)?;
    let query =
        format!("{{ trace(traceId: {trace_id:?}) {{ spans {{ service name attributes }} }} }}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    let mut last = Value::Null;
    loop {
        if let Ok(data) = parallax_graphql(&query).await {
            last = data;
            if inventory_trace_matches(&last, require_postgres) {
                println!("Parallax trace PASS: {label}");
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Parallax trace FAIL: {label} (trace={trace_id}); last response: {last}");
        }
        tokio::time::sleep(Duration::from_secs(poll)).await;
    }
}

async fn inventory_failure() -> anyhow::Result<i32> {
    let base = url_env("INVENTORY_URL", "http://localhost:8089");
    let tenant = std::env::var("INVENTORY_TENANT_ID").unwrap_or_else(|_| "tenant-acme".to_owned());
    let run_id =
        std::env::var("B2_RUN_ID").unwrap_or_else(|_| format!("b2-{}", std::process::id()));
    let url = format!(
        "{base}/reserve?tenant_id={tenant}&reservation_id={run_id}-failure&sku=WIDGET-1&quantity=1&fail=1"
    );
    let (status, body) = request_json(Method::GET, &url, HeaderMap::new(), None).await?;
    ensure!(
        status == StatusCode::SERVICE_UNAVAILABLE,
        "expected inventory 503, got {status}: {body}"
    );
    println!("inventory reservation failure: HTTP {status} {body}");
    Ok(0)
}

async fn recommendation_request(
    sku: &str,
    query: &[(&str, &str)],
    trace_id: Option<&str>,
) -> anyhow::Result<(StatusCode, Value)> {
    let base = url_env("RECOMMENDATION_URL", "http://localhost:8090");
    let mut url = Url::parse(&format!("{base}/recommend"))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair(
            "tenant_id",
            &std::env::var("RECOMMENDATION_TENANT_ID").unwrap_or_else(|_| "tenant-acme".to_owned()),
        );
        pairs.append_pair("sku", sku);
        pairs.append_pair("limit", "8");
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
    }
    let mut headers = HeaderMap::new();
    if let Some(trace_id) = trace_id {
        headers.insert(
            "traceparent",
            format!("00-{trace_id}-{}-01", hex_id(8)).parse()?,
        );
    }
    request_json(Method::GET, url.as_str(), headers, None).await
}

fn ensure_recommendation_response(
    label: &str,
    sku: &str,
    status: StatusCode,
    body: &Value,
    expected_workers: Option<u64>,
) -> anyhow::Result<()> {
    ensure!(
        status.is_success(),
        "{label}: recommendation failed: HTTP {status}: {body}"
    );
    ensure!(
        body.get("sku").and_then(Value::as_str) == Some(sku)
            && body
                .get("tenant_id")
                .and_then(Value::as_str)
                .is_some_and(|tenant| !tenant.is_empty())
            && body.get("source").and_then(Value::as_str) == Some("catalog-graphql")
            && body.get("product").is_some_and(Value::is_object)
            && body.get("products").is_some_and(Value::is_array)
            && body.get("variants").is_some_and(Value::is_array)
            && body.get("recommended").is_some_and(Value::is_array),
        "{label}: response did not prove a catalog-backed recommendation: {body}"
    );
    if let Some(expected_workers) = expected_workers {
        ensure!(
            body.pointer("/chaos/stampede_workers")
                .and_then(Value::as_u64)
                == Some(expected_workers),
            "{label}: response did not prove stampede_workers={expected_workers}: {body}"
        );
    }
    Ok(())
}

async fn recommendation_slow_query() -> anyhow::Result<i32> {
    let (status, body) = recommendation_request("WIDGET-1", &[("slow", "750")], None).await?;
    ensure_recommendation_response("slow recommendation", "WIDGET-1", status, &body, None)?;
    ensure!(
        body.pointer("/chaos/slow_ms").and_then(Value::as_u64) == Some(750),
        "slow recommendation response omitted the 750ms control: {body}"
    );
    println!("recommendation slow query verified: {body}");
    Ok(0)
}

async fn recommendation_stampede() -> anyhow::Result<i32> {
    let sku = std::env::var("SKU").unwrap_or_else(|_| "WIDGET-1".to_owned());
    let mut representative_trace_id = None;
    for index in 1..=10 {
        let trace_id = hex_id(16);
        if representative_trace_id.is_none() {
            representative_trace_id = Some(trace_id.clone());
        }
        let (status, body) = recommendation_request(&sku, &[], Some(&trace_id)).await?;
        ensure_recommendation_response(
            &format!("same-SKU request {index}"),
            &sku,
            status,
            &body,
            None,
        )?;
        ensure_recommendation_normal(&format!("same-SKU request {index}"), &body)?;
    }
    for sku in ["WIDGET-1", "WIDGET-2", "GADGET-1", "GADGET-2"] {
        let (status, body) = recommendation_request(sku, &[], None).await?;
        ensure_recommendation_response(&format!("SKU {sku}"), sku, status, &body, None)?;
        ensure_recommendation_normal(&format!("SKU {sku}"), &body)?;
    }
    let stampede_trace_id = hex_id(16);
    let (status, body) =
        recommendation_request(&sku, &[("stampede", "10")], Some(&stampede_trace_id)).await?;
    ensure_recommendation_response(
        "bounded recommendation stampede",
        &sku,
        status,
        &body,
        Some(10),
    )?;
    println!("recommendation stampede workload verified: {body}");
    if validate_parallax_mode()? {
        wait_for_recommendation_trace(
            representative_trace_id
                .as_deref()
                .context("recommendation representative trace was not created")?,
            "normal recommendation",
            1,
        )
        .await?;
        wait_for_recommendation_trace(&stampede_trace_id, "recommendation stampede", 10).await?;
    }
    Ok(0)
}

fn ensure_recommendation_normal(label: &str, body: &Value) -> anyhow::Result<()> {
    ensure!(
        body.pointer("/chaos/slow_ms").and_then(Value::as_u64) == Some(0)
            && body.pointer("/chaos/leak_kb").and_then(Value::as_u64) == Some(0)
            && body
                .pointer("/chaos/stampede_workers")
                .and_then(Value::as_u64)
                == Some(0),
        "{label}: normal recommendation carried chaos controls: {body}"
    );
    Ok(())
}

async fn wait_for_recommendation_trace(
    trace_id: &str,
    label: &str,
    minimum_catalog_spans: usize,
) -> anyhow::Result<()> {
    let timeout = positive_env("PARALLAX_TRACE_TIMEOUT_SECONDS", 30)?;
    let poll = positive_env("PARALLAX_TRACE_POLL_SECONDS", 1)?;
    let query = format!("{{ trace(traceId: {trace_id:?}) {{ spans {{ service name }} }} }}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    let mut last = Value::Null;
    loop {
        if let Ok(data) = parallax_graphql(&query).await {
            last = data;
            if let Some(spans) = last.pointer("/trace/spans").and_then(Value::as_array) {
                let recommendation_seen = spans.iter().any(|span| {
                    span.get("service").and_then(Value::as_str) == Some("recommendation")
                        && span.get("name").and_then(Value::as_str) == Some("recommend")
                });
                let catalog_spans = spans
                    .iter()
                    .filter(|span| {
                        span.get("service").and_then(Value::as_str) == Some("recommendation")
                            && span.get("name").and_then(Value::as_str)
                                == Some("catalog.graphql.recommendations")
                    })
                    .count();
                if recommendation_seen && catalog_spans >= minimum_catalog_spans {
                    println!("Parallax trace PASS: {label} ({trace_id})");
                    return Ok(());
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Parallax trace FAIL: {label} (trace={trace_id}); last response: {last}");
        }
        tokio::time::sleep(Duration::from_secs(poll)).await;
    }
}

async fn recommendation_cache_leak() -> anyhow::Result<i32> {
    let root = repository_root();
    let compose_file = root.join("deploy/docker-compose.yml");
    run_compose(
        &root,
        &compose_file,
        None,
        "v1",
        &["up", "-d", "flagd", "recommendation"],
    )
    .await?;
    let settle = positive_env("B6_FLAG_SETTLE_SECONDS", 12)?;
    with_flag_variant(&root, "cacheLeak", "on", || async move {
        tokio::time::sleep(Duration::from_secs(settle)).await;
        let requests = positive_env("B6_REQUESTS", 10)?;
        for index in 1..=requests {
            let (status, body) =
                recommendation_request("WIDGET-1", &[("leak", "512")], None).await?;
            ensure_recommendation_response(
                &format!("cache leak request {index}/{requests}"),
                "WIDGET-1",
                status,
                &body,
                None,
            )?;
            ensure!(
                body.pointer("/chaos/leak_kb")
                    .and_then(Value::as_u64)
                    .is_some_and(|value| value >= 256),
                "cache leak request did not retain bounded memory: {body}"
            );
        }
        println!("cache leak workload completed with flagd cacheLeak=on");
        Ok(0)
    })
    .await
}

async fn execution_stack() -> anyhow::Result<i32> {
    let base_attributes = std::env::var("OTEL_RESOURCE_ATTRIBUTES")
        .unwrap_or_else(|_| "deployment.environment.name=playground".to_owned());
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:4317".to_owned());
    let stitched_id =
        nonempty_env("CLI_INVOCATION_ID").unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let orphan_id = uuid::Uuid::new_v4().to_string();
    for (label, invocation_id, orphan) in [
        ("stitched", stitched_id, false),
        ("orphan", orphan_id, true),
    ] {
        let attributes = if base_attributes.contains("cli.invocation.id=") {
            base_attributes.clone()
        } else if base_attributes.trim().is_empty() {
            format!("cli.invocation.id={invocation_id}")
        } else {
            format!("{base_attributes},cli.invocation.id={invocation_id}")
        };
        let mut args = vec![
            "daemon".to_owned(),
            "--session".to_owned(),
            invocation_id.clone(),
        ];
        if orphan {
            args.push("--orphan".to_owned());
        }
        let env = [
            ("CLI_INVOCATION_ID", invocation_id.as_str()),
            ("OTEL_RESOURCE_ATTRIBUTES", attributes.as_str()),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint.as_str()),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            ("PARALLAX_ENV", "playground"),
            ("RUST_LOG", "info"),
        ];
        run_program_with_env(current_executable().await?, args, None, &env).await?;
        println!("execution stack {label} invocation {invocation_id} completed");
    }
    Ok(0)
}

async fn browser_test(spec: &str, grep: &str) -> anyhow::Result<i32> {
    let root = repository_root();
    let compose = spec.contains("compose.");
    let compose_base = url_env("WEB_URL", "http://localhost:5173");
    let mut env = vec![if compose {
        ("PLAYGROUND_COMPOSE_E2E", "1")
    } else {
        ("PLAYGROUND_MOCK_E2E", "1")
    }];
    if compose {
        env.push(("PLAYGROUND_COMPOSE_BASE_URL", compose_base.as_str()));
    }
    run_program_with_env(
        PathBuf::from("bun"),
        vec![
            "x".into(),
            "playwright".into(),
            "test".into(),
            spec.into(),
            "--grep".into(),
            grep.into(),
        ],
        Some(root.join("web")),
        &env,
    )
    .await
}

async fn browser_journey() -> anyhow::Result<i32> {
    let web_url = url_env("WEB_URL", "http://localhost:5173");
    let client = http_client()?;
    for path in [
        "/",
        "/catalog",
        "/products/WIDGET-1",
        "/cart",
        "/checkout",
        "/orders",
        "/analytics",
    ] {
        let response = client.get(format!("{web_url}{path}")).send().await?;
        ensure!(
            response.status().is_success(),
            "browser route {path} failed: {}",
            response.status()
        );
    }
    let root = repository_root();
    let parallax_required = validate_parallax_mode()?;
    let trace_id = hex_id(16);
    let traceparent = format!("00-{trace_id}-{}-01", hex_id(8));
    let tracestate =
        std::env::var("TRACESTATE").unwrap_or_else(|_| "playground=browser".to_owned());
    let baggage = std::env::var("BAGGAGE").unwrap_or_else(|_| {
        "tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal".to_owned()
    });
    let mut env = vec![
        ("PLAYGROUND_COMPOSE_E2E", "1"),
        ("PLAYGROUND_COMPOSE_BASE_URL", web_url.as_str()),
        ("TRACEPARENT", traceparent.as_str()),
        ("TRACESTATE", tracestate.as_str()),
        ("BAGGAGE", baggage.as_str()),
    ];
    let otlp_endpoint = if parallax_required {
        let configured = std::env::var("PLAYGROUND_TEST_OTLP_ENDPOINT")
            .or_else(|_| std::env::var("PARALLAX_OTLP_HTTP_TRACES_ENDPOINT"))
            .unwrap_or_else(|_| {
                format!(
                    "{}/v1/traces",
                    url_env("PARALLAX_URL", "http://127.0.0.1:4318")
                )
            });
        Some(configured)
    } else {
        None
    };
    if let Some(endpoint) = otlp_endpoint.as_deref() {
        env.push(("PLAYGROUND_TEST_OTLP_ENDPOINT", endpoint));
    }
    run_program_with_env(
        PathBuf::from("bun"),
        vec!["run".into(), "e2e:compose".into()],
        Some(root.join("web")),
        &env,
    )
    .await?;
    if parallax_required {
        wait_for_browser_trace(&trace_id).await?;
    }
    Ok(0)
}

async fn wait_for_browser_trace(trace_id: &str) -> anyhow::Result<()> {
    let timeout = positive_env("PARALLAX_TRACE_TIMEOUT_SECONDS", 30)?;
    let poll = positive_env("PARALLAX_TRACE_POLL_SECONDS", 1)?;
    let query = format!(
        "{{ trace(traceId: {trace_id:?}) {{ spans {{ service name attributes resource }} }} }}"
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    let mut last = Value::Null;
    loop {
        if let Ok(data) = parallax_graphql(&query).await {
            last = data;
            if let Some(spans) = last.pointer("/trace/spans").and_then(Value::as_array) {
                let has = |service: &str| {
                    spans
                        .iter()
                        .any(|span| span.get("service").and_then(Value::as_str) == Some(service))
                };
                if has("playground-web-tests") && has("web") && has("storefront") && has("checkout")
                {
                    println!("Parallax browser trace PASS: {trace_id}");
                    return Ok(());
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Parallax browser trace FAIL: {trace_id}; last response: {last}");
        }
        tokio::time::sleep(Duration::from_secs(poll)).await;
    }
}

async fn typed_events() -> anyhow::Result<i32> {
    checkout_expected(
        "typed checkout completed",
        "WIDGET-1",
        "tok_visa",
        "typed-success",
        &[],
        StatusCode::OK,
    )
    .await?;
    checkout_expected(
        "typed payment declined",
        "WIDGET-1",
        "tok_decline",
        "typed-decline",
        &[],
        StatusCode::PAYMENT_REQUIRED,
    )
    .await?;
    synthetic_order_request(
        "typed synthetic order",
        "d4c3b2a19087654321fedcba98765432",
        3,
        false,
        false,
        0,
    )
    .await?;
    let (status, body) =
        graphql_request("query typedEventsProducts { products { items { id sku name } } }").await?;
    ensure!(
        status.is_success(),
        "typed catalog products failed: HTTP {status}: {body}"
    );
    ensure_graphql_success("typed catalog products", &body)?;
    println!("typed Rust/Java/HTTP event workload completed");
    Ok(0)
}

async fn k6_load(extra: &[String]) -> anyhow::Result<i32> {
    let root = repository_root();
    let storefront_url = nonempty_env("STOREFRONT_URL")
        .unwrap_or_else(|| "http://localhost:8094/graphql".to_owned());
    let loadgen_run_id =
        nonempty_env("LOADGEN_RUN_ID").unwrap_or_else(|| format!("b16-{}", uuid::Uuid::new_v4()));
    let mut args = vec![
        "run".into(),
        root.join("loadgen/checkout.ts").display().to_string(),
    ];
    args.extend(extra.iter().cloned());
    println!("B16 Storefront load: run={loadgen_run_id} url={storefront_url}");
    let env = [
        ("STOREFRONT_URL", storefront_url.as_str()),
        ("LOADGEN_RUN_ID", loadgen_run_id.as_str()),
    ];
    run_program_with_env(PathBuf::from("k6"), args, Some(root), &env).await
}

async fn sustained_checkout_failures() -> anyhow::Result<i32> {
    let duration = positive_float_env("BREACH_SECONDS", 200.0)?;
    let gap = positive_float_env("REQUEST_GAP_SECONDS", 2.0)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(duration);
    let mut count = 0_u64;
    while tokio::time::Instant::now() < deadline {
        checkout_expected(
            &format!("checkout error-rate breach #{count}"),
            "WIDGET-1",
            "tok_decline",
            &format!("error-rate-breach-{count}"),
            &[],
            StatusCode::PAYMENT_REQUIRED,
        )
        .await?;
        count += 1;
        tokio::time::sleep(Duration::from_secs_f64(gap)).await;
    }
    println!("checkout error-rate breach traffic completed: {count} requests");
    Ok(0)
}

async fn sustained_recommendation() -> anyhow::Result<i32> {
    let duration = positive_float_env("BREACH_SECONDS", 200.0)?;
    let gap = positive_float_env("REQUEST_GAP_SECONDS", 2.0)?;
    let slow_ms = positive_env("BREACH_SLOW_MS", 900)?;
    let slow_query = slow_ms.to_string();
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(duration);
    let mut count = 0_u64;
    while tokio::time::Instant::now() < deadline {
        let (status, body) =
            recommendation_request("WIDGET-1", &[("slow", slow_query.as_str())], None).await?;
        ensure!(
            status.is_success(),
            "recommendation p95 breach request failed: HTTP {status}: {body}"
        );
        count += 1;
        tokio::time::sleep(Duration::from_secs_f64(gap)).await;
    }
    println!("recommendation p95 breach traffic completed: {count} requests at {slow_ms}ms");
    Ok(0)
}

async fn recovery_traffic() -> anyhow::Result<i32> {
    let settle = positive_env("FLAG_SETTLE_SECONDS", 12)?;
    let duration = positive_float_env("RECOVER_SECONDS", 200.0)?;
    let gap = positive_float_env("REQUEST_GAP_SECONDS", 2.0)?;
    tokio::time::sleep(Duration::from_secs(settle)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(duration);
    let mut count = 0_u64;
    while tokio::time::Instant::now() < deadline {
        checkout_expected(
            &format!("healthy checkout recovery #{count}"),
            "WIDGET-1",
            "tok_visa",
            &format!("recovery-{count}"),
            &[],
            StatusCode::OK,
        )
        .await?;
        let (status, body) = recommendation_request("WIDGET-1", &[], None).await?;
        ensure!(
            status.is_success(),
            "healthy recommendation recovery failed: HTTP {status}: {body}"
        );
        count += 1;
        tokio::time::sleep(Duration::from_secs_f64(gap)).await;
    }
    println!("healthy checkout and recommendation recovery traffic completed: {count} rounds");
    Ok(0)
}

async fn sampling_gap() -> anyhow::Result<i32> {
    let root = repository_root();
    let configured_file =
        std::env::var("COMPOSE_FILE").unwrap_or_else(|_| "deploy/docker-compose.yml".to_owned());
    let compose_file = PathBuf::from(&configured_file);
    let compose_file = if compose_file.is_absolute() {
        compose_file
    } else {
        root.join(compose_file)
    };
    let base = url_env("CHECKOUT_BASE", "http://localhost:8088");
    let args = vec![
        "compose".to_owned(),
        "-f".to_owned(),
        compose_file.display().to_string(),
        "up".to_owned(),
        "-d".to_owned(),
        "--no-deps".to_owned(),
        "--force-recreate".to_owned(),
        "checkout".to_owned(),
    ];
    let restore_args = args.clone();
    let lowered_env = [("PLAYGROUND_SAMPLE_RATIO", "0.1")];
    let lifecycle = async {
        run_program_with_env(
            PathBuf::from("docker"),
            args.clone(),
            Some(root.clone()),
            &lowered_env,
        )
        .await?;
        wait_for_checkout(&base).await?;
        let mut successful = 0_u64;
        for index in 1..=50_u64 {
            let quantity = index % 5 + 1;
            let (status, body) = checkout_http(
                "WIDGET-1",
                quantity,
                "tok_visa",
                &format!("sampling-gap-{index}"),
                HeaderMap::new(),
                &[],
            )
            .await?;
            if status == StatusCode::OK {
                successful += 1;
            }
            println!("sampling checkout {index}/50: HTTP {status} {body}");
        }
        println!("sampling gap drove {successful}/50 successful requests at 10% root sampling");
        Ok::<_, anyhow::Error>(0)
    }
    .await;
    let restored = std::env::var("PLAYGROUND_SAMPLE_RATIO").unwrap_or_default();
    let restore_env = [("PLAYGROUND_SAMPLE_RATIO", restored.as_str())];
    let restore = run_program_with_env(
        PathBuf::from("docker"),
        restore_args,
        Some(root),
        &restore_env,
    )
    .await;
    match (lifecycle, restore) {
        (Ok(code), Ok(_)) => Ok(code),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error.context("sampling cleanup failed")),
        (Err(error), Err(cleanup)) => {
            bail!("sampling gap failed: {error}; cleanup failed: {cleanup}")
        }
    }
}

async fn cron_suite() -> anyhow::Result<i32> {
    let settle = positive_env("CRON_SUITE_SETTLE_SECONDS", 5)?;
    for (slot, mode) in [
        (1_u64, "ok"),
        (2, "ok"),
        (3, "fail"),
        (4, "stuck"),
        (5, "missed"),
        (6, "duplicate"),
    ] {
        let invocation_id = format!("playground-report-suite-slot-{slot}");
        if mode == "missed" {
            println!("slot {slot}: missed (no process telemetry emitted)");
            tokio::time::sleep(Duration::from_secs(settle)).await;
            continue;
        }
        let args = ["cron", mode];
        let env = [("CLI_INVOCATION_ID", invocation_id.as_str())];
        let (code, output) = run_current_allow_failure_with_env(&args, &env).await?;
        ensure!(
            code == 0 || (mode == "fail" && code == 1),
            "cron {:?} returned {code}: {output}",
            args
        );
        print!("{output}");
        println!("slot {slot} exit={code} invocation={invocation_id}");
        tokio::time::sleep(Duration::from_secs(settle)).await;
    }
    Ok(0)
}

async fn run_shape(id: &str) -> anyhow::Result<i32> {
    shapes::run(vec![id.to_owned()]).await
}

async fn journey(args: &[&str]) -> anyhow::Result<i32> {
    let mut command = vec!["console".to_owned()];
    command.extend(args.iter().map(|arg| (*arg).to_owned()));
    run_program(current_executable().await?, command, None).await
}

async fn expected_journey_failure() -> anyhow::Result<i32> {
    let (code, output) =
        run_current_allow_failure(&["console", "--seconds", "6", "--fail-at", "checkout.submit"])
            .await?;
    ensure!(
        code == 1,
        "forced journey failure returned {code}: {output}"
    );
    ensure!(
        output.contains("checkout.submit"),
        "forced journey did not emit checkout.submit: {output}"
    );
    Ok(0)
}

async fn parallel_journeys() -> anyhow::Result<i32> {
    let mut children = JoinSet::new();
    for _ in 0..3 {
        let executable = current_executable().await?;
        children.spawn(async move {
            Command::new(executable)
                .args(["console", "--seconds", "6"])
                .status()
                .await
                .context("parallel console failed to start")
        });
    }
    run_current(&["daemon"]).await?;
    while let Some(result) = children.join_next().await {
        let status = result??;
        ensure!(status.success(), "parallel console exited unsuccessfully");
    }
    Ok(0)
}

async fn grpc_error_corpus() -> anyhow::Result<i32> {
    checkout_saga().await?;
    let base = url_env("CHECKOUT_URL", "http://localhost:8088");
    let request = json!({
        "tenant_id": "tenant-acme",
        "customer_id": "customer-acme-ava",
        "currency_code": "USD",
        "payment_method_token": "tok_visa",
        "payment_method_type": "card",
        "items": [{"sku": "", "quantity": 0}]
    });
    let (status, body) = request_json(
        Method::POST,
        &format!("{base}/checkout"),
        json_headers(),
        Some(request),
    )
    .await?;
    ensure!(
        status == StatusCode::BAD_REQUEST,
        "invalid checkout returned {status}: {body}"
    );
    println!("invalid checkout: {body}");
    grpc_deadline_retry().await?;
    Ok(0)
}

async fn ecosystem_full() -> anyhow::Result<i32> {
    checkout_saga().await?;
    storefront_graphql("pricing").await?;
    storefront_graphql("catalog").await?;
    seeded_order_replay().await?;
    browser_journey().await?;
    run_current(&[]).await
}

fn parse_invocation_id(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            line.split_once("Parallax invocation id:")
                .map(|(_, rest)| rest)
        })
        .and_then(|rest| rest.split_whitespace().next())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn parse_json_output(text: &str) -> Option<Value> {
    serde_json::from_str(text.trim()).ok().or_else(|| {
        text.lines()
            .rev()
            .find_map(|line| serde_json::from_str(line.trim()).ok())
    })
}

async fn issue_context() -> anyhow::Result<i32> {
    emit_issue_seed().await?;
    let fingerprint = wait_for_issue().await?;
    let bundle = parallax_graphql(&format!(
        "{{ bundle(fingerprint: {fingerprint:?}) {{ canonicalHash markdown json }} }}"
    ))
    .await?;
    let bundle = bundle
        .get("bundle")
        .context("bundle response is missing bundle")?;
    let hash = bundle
        .get("canonicalHash")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("issue bundle has no canonical hash")?;
    let markdown = bundle.get("markdown").and_then(Value::as_str).unwrap_or("");
    let encoded = bundle.get("json").map(Value::to_string).unwrap_or_default();
    ensure!(
        markdown.len() > 20 && encoded.len() > 20,
        "issue bundle evidence is unexpectedly empty"
    );

    let bin = parallax_bin();
    let (code, context) = capture_program(
        bin.clone(),
        vec![
            "issue".into(),
            "context".into(),
            fingerprint.clone(),
            "--format".into(),
            "json".into(),
        ],
        None,
    )
    .await?;
    ensure!(code == 0, "parallax issue context failed: {context}");
    ensure!(
        context.contains("bundle")
            || context.contains("canonical")
            || context.contains("schema")
            || context.len() > 40,
        "issue context returned no evidence"
    );
    run_program(
        bin,
        vec!["issue".into(), "resolve".into(), fingerprint.clone()],
        None,
    )
    .await?;
    println!("issue context verified fingerprint={fingerprint} hash={hash}");
    Ok(0)
}

async fn invocation_lifecycle() -> anyhow::Result<i32> {
    let bin = parallax_bin();
    let (code, output) = capture_program(
        bin.clone(),
        vec![
            "invocation".into(),
            "start".into(),
            "--".into(),
            "/bin/echo".into(),
            "invocation-lifecycle".into(),
        ],
        None,
    )
    .await?;
    ensure!(code == 0, "invocation start failed: {output}");
    let invocation_id = if let Some(id) = parse_invocation_id(&output) {
        id
    } else {
        let (list_code, listed) = capture_program(
            bin.clone(),
            vec![
                "invocation".into(),
                "list".into(),
                "--format".into(),
                "json".into(),
            ],
            None,
        )
        .await?;
        ensure!(list_code == 0, "invocation list failed: {listed}");
        parse_json_output(&listed)
            .and_then(|value| {
                value
                    .as_array()
                    .and_then(|rows| rows.first())
                    .or_else(|| {
                        value
                            .get("items")
                            .and_then(Value::as_array)
                            .and_then(|rows| rows.first())
                    })
                    .or_else(|| {
                        value
                            .get("invocations")
                            .and_then(Value::as_array)
                            .and_then(|rows| rows.first())
                    })
                    .and_then(|row| row.get("id").or_else(|| row.get("invocationId")))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .context("invocation output contained no invocation id")?
    };
    for args in [
        vec!["invocation".into(), "inspect".into(), invocation_id.clone()],
        vec![
            "invocation".into(),
            "bundle".into(),
            invocation_id.clone(),
            "--format".into(),
            "json".into(),
        ],
    ] {
        let (child_code, child_output) = capture_program(bin.clone(), args, None).await?;
        ensure!(
            child_code == 0,
            "invocation lifecycle command failed: {child_output}"
        );
    }
    println!("invocation lifecycle verified id={invocation_id}");
    Ok(0)
}

async fn live_tail() -> anyhow::Result<i32> {
    let base = url_env("PARALLAX_URL", "http://127.0.0.1:4000");
    let client = http_client()?;
    let mut request = client.get(format!("{base}/v1/traces/stream"));
    if let Some(token) = std::env::var("PARALLAX_API_TOKEN")
        .ok()
        .filter(|token| !token.trim().is_empty())
    {
        request = request.bearer_auth(token);
    }
    let stream_task = tokio::spawn(async move {
        let response = request.send().await.context("trace SSE request failed")?;
        ensure!(
            response.status().is_success(),
            "trace SSE returned {}",
            response.status()
        );
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk.context("trace SSE chunk failed")?);
            if !bytes.is_empty() {
                return Ok::<_, anyhow::Error>(bytes);
            }
        }
        bail!("trace SSE closed without data")
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    emit_issue_seed().await?;
    let bytes = match tokio::time::timeout(Duration::from_secs(8), stream_task).await {
        Ok(joined) => joined??,
        Err(_) => bail!("trace SSE produced no data before timeout"),
    };
    println!("live trace tail verified bytes={}", bytes.len());
    Ok(0)
}

async fn alerting() -> anyhow::Result<i32> {
    let destination = parallax_graphql(
        r#"mutation { alertDestinationSave(name: "rust-c4-hook", kind: "webhook", config: "{\"url\":\"http://127.0.0.1:9/rust-c4\"}") { id } }"#,
    )
    .await?;
    let destination_id = destination
        .pointer("/alertDestinationSave/id")
        .and_then(Value::as_str)
        .context("alert destination mutation omitted id")?;
    let slack = parallax_graphql(
        r#"mutation { alertDestinationSave(name: "rust-c4-slack", kind: "slack_webhook", config: "{\"url\":\"http://127.0.0.1:9/rust-c4-slack\"}") { id } }"#,
    )
    .await?;
    let slack_id = slack
        .pointer("/alertDestinationSave/id")
        .and_then(Value::as_str)
        .context("slack destination mutation omitted id")?;
    let rule_query = format!(
        r#"mutation {{ alertRuleSave(input: {{ name: "rust-c4-high-errors", enabled: true, signalType: "error_rate", services: ["checkout"], comparator: "gt", threshold: 0.2, windowMinutes: 5, minimumSampleCount: 1, consecutiveBreachesRequired: 1, consecutiveHealthyRequired: 1, severity: "critical", destinationIds: ["{destination_id}"] }}) {{ id }} }}"#
    );
    let rule = parallax_graphql(&rule_query).await?;
    let rule_id = rule
        .pointer("/alertRuleSave/id")
        .and_then(Value::as_str)
        .context("alert rule mutation omitted id")?;
    emit_issue_seed().await?;
    let timeout = std::env::var("PARALLAX_ALERT_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(180);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    loop {
        let incidents = parallax_graphql("{ alertIncidents(limit: 8) { id status } }")
            .await
            .unwrap_or_else(|_| json!({}));
        if let Some(id) = incidents
            .pointer("/alertIncidents")
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row.get("status").and_then(Value::as_str) == Some("open"))
            })
            .and_then(|row| row.get("id"))
            .and_then(Value::as_str)
        {
            println!(
                "alert incident verified id={id} rule={rule_id} destination={destination_id} slack={slack_id}"
            );
            return Ok(0);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("alerting produced no open incident");
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn saved_state() -> anyhow::Result<i32> {
    let dashboard = parallax_graphql(
        r#"mutation { dashboardSave(name: "rust-c5-dash", layout: "{\"widgets\":[]}") { id name } }"#,
    )
    .await?;
    let investigation = parallax_graphql(
        r#"mutation { investigationSave(name: "rust-c5-case", state: "{\"version\":1}") { id name } }"#,
    )
    .await?;
    let sql = parallax_graphql(r#"{ sql(query: "SELECT 1") { columns rowCount } }"#).await?;
    ensure!(
        dashboard
            .pointer("/dashboardSave/id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "dashboard save returned no id"
    );
    ensure!(
        investigation
            .pointer("/investigationSave/id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty()),
        "investigation save returned no id"
    );
    ensure!(sql.get("sql").is_some(), "SQL surface returned no data");
    println!("saved dashboard, investigation, and SQL state verified");
    Ok(0)
}

fn hmac_sha256_hex(secret: &str, body: &[u8]) -> anyhow::Result<String> {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .context("could not initialize GitHub HMAC")?;
    mac.update(body);
    Ok(mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

async fn raw_request(
    method: Method,
    url: &str,
    headers: HeaderMap,
    body: Vec<u8>,
) -> anyhow::Result<(StatusCode, String)> {
    let response = http_client()?
        .request(method, url)
        .headers(headers)
        .body(body)
        .send()
        .await
        .with_context(|| format!("request failed: {url}"))?;
    let status = response.status();
    let text = response.text().await.context("failed to read response")?;
    Ok((status, text))
}

async fn github_ingest() -> anyhow::Result<i32> {
    let payload_path = repository_root().join("fixtures/github/deployment.json");
    let payload = fs::read(&payload_path)
        .with_context(|| format!("missing GitHub fixture: {}", payload_path.display()))?;
    let secret = std::env::var("GITHUB_WEBHOOK_SECRET")
        .unwrap_or_else(|_| "playground-c6-secret".to_owned());
    let signature = format!("sha256={}", hmac_sha256_hex(&secret, &payload)?);
    let base = url_env("PARALLAX_URL", "http://127.0.0.1:4000");
    let mut good_headers = parallax_headers()?;
    good_headers.insert("content-type", "application/json".parse()?);
    good_headers.insert("x-github-event", "deployment".parse()?);
    good_headers.insert(
        "x-github-delivery",
        "11111111-2222-3333-4444-555555555555".parse()?,
    );
    good_headers.insert("x-hub-signature-256", signature.parse()?);
    let (good, _) = raw_request(
        Method::POST,
        &format!("{base}/webhooks/github"),
        good_headers,
        payload.clone(),
    )
    .await?;
    let mut bad_headers = parallax_headers()?;
    bad_headers.insert("content-type", "application/json".parse()?);
    bad_headers.insert("x-github-event", "deployment".parse()?);
    bad_headers.insert(
        "x-github-delivery",
        "11111111-2222-3333-4444-555555555556".parse()?,
    );
    bad_headers.insert("x-hub-signature-256", "sha256=deadbeef".parse()?);
    let (bad, _) = raw_request(
        Method::POST,
        &format!("{base}/webhooks/github"),
        bad_headers,
        payload,
    )
    .await?;
    ensure!(good.is_success(), "valid GitHub signature rejected: {good}");
    ensure!(
        !bad.is_success(),
        "invalid GitHub signature accepted: {bad}"
    );
    println!("GitHub deployment webhook verified valid={good} invalid={bad}");
    Ok(0)
}

async fn agent_session() -> anyhow::Result<i32> {
    let root = repository_root();
    let fixture = root.join("fixtures/claude-code/session.ndjson");
    ensure!(
        fixture.is_file(),
        "Claude session fixture is missing: {}",
        fixture.display()
    );
    let bin = parallax_bin();
    let mcp = parallax_mcp();
    let (import_code, import_output) = capture_program(
        bin.clone(),
        vec![
            "import-claude".into(),
            fixture.display().to_string(),
            "--json".into(),
        ],
        None,
    )
    .await?;
    ensure!(
        import_code == 0,
        "Claude session import failed: {import_output}"
    );
    let import = parse_json_output(&import_output).context("Claude import returned no JSON")?;
    let import_id = import
        .get("import_id")
        .and_then(Value::as_str)
        .or_else(|| {
            import
                .pointer("/session/session_id")
                .and_then(Value::as_str)
        })
        .filter(|value| !value.is_empty())
        .context("Claude import returned no session id")?;

    let fingerprint = match wait_for_issue().await {
        Ok(fingerprint) => fingerprint,
        Err(_) => {
            emit_issue_seed().await?;
            wait_for_issue().await?
        }
    };
    let (invocation_code, invocation_output) = capture_program(
        bin.clone(),
        vec![
            "invocation".into(),
            "start".into(),
            "--".into(),
            "/bin/echo".into(),
            "agent-session".into(),
        ],
        None,
    )
    .await?;
    ensure!(
        invocation_code == 0,
        "agent invocation failed: {invocation_output}"
    );
    let invocation_id =
        parse_invocation_id(&invocation_output).context("agent invocation returned no id")?;

    let (mcp_code, mcp_output) = capture_program(
        mcp.clone(),
        vec![
            "--url".into(),
            url_env("PARALLAX_URL", "http://127.0.0.1:4000"),
            "check".into(),
            "--fingerprint".into(),
            fingerprint.clone(),
            "--parallax-bin".into(),
            bin.display().to_string(),
        ],
        None,
    )
    .await?;
    ensure!(mcp_code == 0, "MCP equivalence check failed: {mcp_output}");
    let session_query =
        format!("{{ agentSession(invocationId: {invocation_id:?}) {{ errorCount truncated }} }}");
    let session = parallax_graphql(&session_query).await?;
    ensure!(
        session.get("agentSession").is_some(),
        "agent session GraphQL projection returned no data"
    );
    let (help_code, help) = capture_program(mcp, vec!["--help".into()], None).await?;
    ensure!(help_code == 0, "parallax-mcp help failed: {help}");
    ensure!(
        help.contains("allow-local-stdio"),
        "MCP help omitted local-stdio trust control"
    );
    println!("agent session verified import={import_id} invocation={invocation_id}");
    Ok(0)
}

async fn lifecycle_ops() -> anyhow::Result<i32> {
    let home = std::env::temp_dir().join(format!("parallax-c9-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(home.join(".parallax"))?;
    let marker = home.join(".parallax/marker");
    fs::write(&marker, b"c9-marker\n")?;
    let real_marker = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .map(|path| path.join(".parallax/marker"))
        .filter(|path| path.is_file())
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok());
    let result = lifecycle_ops_in_home(&home, &marker, real_marker).await;
    let cleanup = fs::remove_dir_all(&home);
    if let Err(error) = cleanup
        && result.is_ok()
    {
        return Err(error.into());
    }
    result
}

async fn lifecycle_ops_in_home(
    home: &Path,
    marker: &Path,
    real_marker_before: Option<std::time::SystemTime>,
) -> anyhow::Result<i32> {
    let home = home.to_str().context("temporary HOME is not UTF-8")?;
    let bin = parallax_bin();
    let env = [("HOME", home)];
    run_program_with_env(bin.clone(), vec!["doctor".into()], None, &env).await?;
    let (plan_code, plan) = capture_program_with_env(
        bin.clone(),
        vec!["prune".into(), "--json".into()],
        None,
        &env,
    )
    .await?;
    ensure!(plan_code == 0, "prune dry-run failed: {plan}");
    ensure!(
        plan.contains("plan_id") || plan.contains("items") || plan.contains("dry"),
        "prune dry-run returned no plan"
    );
    run_program_with_env(
        bin.clone(),
        vec![
            "prune".into(),
            "--execute".into(),
            "--yes".into(),
            "--json".into(),
        ],
        None,
        &env,
    )
    .await?;
    ensure!(marker.is_file(), "isolated prune removed the marker");
    run_program_with_env(
        bin.clone(),
        vec![
            "context".into(),
            "add".into(),
            "c9lab".into(),
            "--url".into(),
            url_env("PARALLAX_URL", "http://127.0.0.1:4000"),
        ],
        None,
        &env,
    )
    .await?;
    let (list_code, listed) = capture_program_with_env(
        bin.clone(),
        vec!["context".into(), "list".into()],
        None,
        &env,
    )
    .await?;
    ensure!(
        list_code == 0 && listed.contains("c9lab"),
        "context list missed c9lab: {listed}"
    );
    run_program_with_env(
        bin.clone(),
        vec!["context".into(), "show".into(), "c9lab".into()],
        None,
        &env,
    )
    .await?;
    let (invocation_code, invocation) = capture_program_with_env(
        bin,
        vec![
            "invocation".into(),
            "start".into(),
            "--otlp-forward".into(),
            "off".into(),
            "--".into(),
            "/bin/echo".into(),
            "c9-otlp-forward".into(),
        ],
        None,
        &env,
    )
    .await?;
    ensure!(
        invocation_code == 0
            && (invocation.contains("invocation id") || invocation.contains("c9-otlp-forward")),
        "isolated invocation forwarding check failed: {invocation}"
    );
    if let Some(before) = real_marker_before {
        let real_marker = std::env::var("HOME")
            .ok()
            .map(PathBuf::from)
            .map(|path| path.join(".parallax/marker"));
        if let Some(path) = real_marker.filter(|path| path.is_file()) {
            let after = fs::metadata(path)?.modified()?;
            ensure!(
                before == after,
                "operator HOME marker changed during isolated prune"
            );
        }
    }
    println!("isolated HOME lifecycle verified");
    Ok(0)
}

async fn sentry_canary() -> anyhow::Result<i32> {
    let base = url_env("PARALLAX_URL", "http://127.0.0.1:4000");
    let event_id = uuid::Uuid::new_v4().simple().to_string();
    let event = json!({
        "event_id": event_id,
        "timestamp": "2026-09-04T00:00:00Z",
        "platform": "rust",
        "message": "parallax redaction canary",
        "extra": {
            "email": CANARY_EMAIL,
            "token": CANARY_TOKEN,
            "card": CANARY_CARD,
            "jwt": CANARY_JWT
        }
    })
    .to_string();
    let header = json!({
        "event_id": event_id,
        "sent_at": "2026-09-04T00:00:00Z"
    })
    .to_string();
    let item = json!({"type": "event", "length": event.len()}).to_string();
    let envelope = format!("{header}\n{item}\n{event}");
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/x-sentry-envelope".parse()?);
    headers.insert(
        "x-sentry-auth",
        "Sentry sentry_key=c8public, sentry_version=7".parse()?,
    );
    let (status, _) = raw_request(
        Method::POST,
        &format!("{base}/api/1/envelope/"),
        headers,
        envelope.into_bytes(),
    )
    .await?;
    ensure!(status.is_success(), "Sentry canary rejected: {status}");
    println!("redaction canary sent");
    Ok(0)
}

async fn sentry_envelopes() -> anyhow::Result<i32> {
    let root = repository_root();
    let dsn = std::env::var("SENTRY_DSN")
        .unwrap_or_else(|_| "http://c8public@127.0.0.1:4000/1".to_owned());
    run_program_with_env(
        PathBuf::from("cargo"),
        vec![
            "run".into(),
            "--locked".into(),
            "-p".into(),
            "playground-telemetry".into(),
            "--example".into(),
            "c8_sentry_emit".into(),
        ],
        Some(root.clone()),
        &[("SENTRY_DSN", dsn.as_str())],
    )
    .await?;
    let java_wrapper = root.join("services/catalog/gradle/wrapper/gradle-wrapper.jar");
    ensure!(
        java_wrapper.exists(),
        "C8 Java proof requires {}",
        java_wrapper.display()
    );
    run_program_with_env(
        PathBuf::from("java"),
        vec![
            "-cp".into(),
            java_wrapper.display().to_string(),
            "org.gradle.wrapper.GradleWrapperMain".into(),
            "--no-daemon".into(),
            "c8SentryEmit".into(),
        ],
        Some(root.join("services/catalog")),
        &[("SENTRY_DSN", dsn.as_str())],
    )
    .await?;
    run_bun_script_with_env(
        "scenarios/c8-emit-js.ts",
        &[],
        &[("SENTRY_DSN", dsn.as_str())],
    )
    .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let data = parallax_graphql("{ issues(limit: 50) { items { title errorType } } }")
            .await
            .unwrap_or_else(|_| json!({}));
        let encoded = data.to_string().to_ascii_lowercase();
        if ["c8-rust-sdk", "c8-java-sdk", "c8-js-sdk"]
            .iter()
            .all(|needle| encoded.contains(needle))
        {
            println!("Sentry Rust/Java/JavaScript envelopes verified");
            return Ok(0);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Sentry envelope verification missing one or more SDK issues: {encoded}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn webhook_capture(
    State(body): State<Arc<Mutex<Option<Vec<u8>>>>>,
    request: Request,
) -> &'static str {
    if let Ok(bytes) = to_bytes(request.into_body(), 1_048_576).await
        && let Ok(mut captured) = body.lock()
    {
        *captured = Some(bytes.to_vec());
    }
    "ok"
}

async fn wait_for_issue_containing(needle: &str) -> anyhow::Result<String> {
    let timeout = std::env::var("PARALLAX_TRACE_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(30);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    loop {
        let data = parallax_graphql(
            "{ issues(limit: 20) { items { fingerprint title latestEvent { message } } } }",
        )
        .await
        .unwrap_or_else(|_| json!({}));
        if let Some(row) = data
            .pointer("/issues/items")
            .and_then(Value::as_array)
            .and_then(|rows| {
                rows.iter().find(|row| {
                    let text = row.to_string().to_ascii_lowercase();
                    text.contains(&needle.to_ascii_lowercase())
                })
            })
            && let Some(fingerprint) = row.get("fingerprint").and_then(Value::as_str)
        {
            return Ok(fingerprint.to_owned());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Parallax produced no issue containing {needle:?}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn redaction_egress() -> anyhow::Result<i32> {
    let body = Arc::new(Mutex::new(None));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("could not bind redaction webhook listener")?;
    let port = listener.local_addr()?.port();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server = serve(
        listener,
        Router::new()
            .fallback(webhook_capture)
            .with_state(body.clone()),
    )
    .with_graceful_shutdown(async {
        let _ = shutdown_rx.await;
    });
    let server_task = tokio::spawn(server.into_future());
    let result = async {
        let destination_query = format!(
            r#"mutation {{ alertDestinationSave(name: "rust-c10-hook", kind: "webhook", config: "{{\"url\":\"http://127.0.0.1:{port}/c10\"}}") {{ id }} }}"#
        );
        let destination = parallax_graphql(&destination_query).await?;
        let destination_id = destination
            .pointer("/alertDestinationSave/id")
            .and_then(Value::as_str)
            .context("redaction destination mutation omitted id")?;
        sentry_canary().await?;
        let fingerprint = wait_for_issue_containing("parallax redaction canary").await?;

        let bundle = parallax_graphql(&format!(
            "{{ bundle(fingerprint: {fingerprint:?}) {{ markdown json }} }}"
        ))
        .await?;
        assert_no_canary("bundle", &bundle.to_string())?;

        let bin = parallax_bin();
        let (cli_code, cli_output) = capture_program(
            bin.clone(),
            vec![
                "issue".into(),
                "context".into(),
                fingerprint.clone(),
                "--format".into(),
                "json".into(),
            ],
            None,
        )
        .await?;
        ensure!(cli_code == 0, "redaction issue context failed: {cli_output}");
        assert_no_canary("cli-issue-context", &cli_output)?;

        let mcp = parallax_mcp();
        let (mcp_code, mcp_output) = capture_program(
            mcp,
            vec![
                "--url".into(),
                url_env("PARALLAX_URL", "http://127.0.0.1:4000"),
                "check".into(),
                "--fingerprint".into(),
                fingerprint.clone(),
                "--parallax-bin".into(),
                bin.display().to_string(),
            ],
            None,
        )
        .await?;
        ensure!(mcp_code == 0, "redaction MCP check failed: {mcp_output}");
        assert_no_canary("mcp-check", &mcp_output)?;

        let ui = parallax_graphql(&format!(
            "{{ issue(fingerprint: {fingerprint:?}) {{ title errorType culprit latestEvent {{ message }} }} logs(limit: 20, query: \"canary\") {{ body service }} }}"
        ))
        .await?;
        assert_no_canary("ui-graphql", &ui.to_string())?;

        let event_id = "c10c10c10c10c10c10c10c10c10c10c1";
        let event = json!({
            "event_id": event_id,
            "timestamp": "2026-09-06T00:00:00Z",
            "platform": "rust",
            "message": "c10 sentry acknowledgement",
            "extra": {"email": CANARY_EMAIL}
        })
        .to_string();
        let envelope = format!(
            "{}\n{}\n{}",
            json!({"event_id": event_id, "sent_at": "2026-09-06T00:00:00Z"}),
            json!({"type": "event", "length": event.len()}),
            event
        );
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/x-sentry-envelope".parse()?);
        headers.insert(
            "x-sentry-auth",
            "Sentry sentry_key=c8public, sentry_version=7".parse()?,
        );
        let (ack_status, ack) = raw_request(
            Method::POST,
            &format!("{}/api/1/envelope/", url_env("PARALLAX_URL", "http://127.0.0.1:4000")),
            headers,
            envelope.into_bytes(),
        )
        .await?;
        ensure!(ack_status.is_success(), "Sentry acknowledgement failed: {ack_status}");
        assert_no_canary("sentry-ack", &ack)?;

        tokio::time::sleep(Duration::from_secs(2)).await;
        if let Some(webhook) = body.lock().ok().and_then(|captured| captured.clone()) {
            assert_no_canary("webhook", &String::from_utf8_lossy(&webhook))?;
        } else {
            println!("redaction webhook had no delivery during the bounded window");
        }
        println!("redaction egress verified fingerprint={fingerprint} destination={destination_id}");
        Ok::<_, anyhow::Error>(0)
    }
    .await;
    let _ = shutdown_tx.send(());
    let _ = server_task.await;
    result
}

async fn ui_agent_verify() -> anyhow::Result<i32> {
    let base = url_env("PARALLAX_URL", "http://127.0.0.1:4000");
    let session = if let Some(session) = std::env::var("AGENT_BROWSER_SESSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        session
    } else {
        let (code, output) = capture_program(
            PathBuf::from("agent-browser"),
            vec![
                "session".into(),
                "id".into(),
                "--scope".into(),
                "worktree".into(),
                "--prefix".into(),
                "c11ui".into(),
            ],
            None,
        )
        .await?;
        ensure!(code == 0, "agent-browser session creation failed: {output}");
        output
            .split_whitespace()
            .last()
            .map(str::to_owned)
            .context("agent-browser returned no session id")?
    };
    let browser = PathBuf::from("agent-browser");
    let mut viewport = vec![
        "--session".into(),
        session.clone(),
        "set".into(),
        "viewport".into(),
    ];
    viewport.extend(["1440".into(), "900".into()]);
    let (code, output) = capture_program(browser.clone(), viewport, None).await?;
    ensure!(code == 0, "agent-browser viewport failed: {output}");
    for (path, expected) in [
        ("/", "Overview"),
        ("/issues", "Issues"),
        ("/traces", "Traces"),
        ("/logs", "Logs"),
        ("/metrics", "Metrics"),
        ("/services", "Services"),
        ("/ecosystem", "Ecosystem"),
        ("/invocations", "CLI Apps|Invocations"),
        ("/alerts", "Alerts"),
        ("/dashboards", "Dashboards"),
        ("/investigations", "Investigations"),
        ("/sql", "SQL"),
        ("/tests", "Tests"),
    ] {
        let (open_code, open_output) = capture_program(
            browser.clone(),
            vec![
                "--session".into(),
                session.clone(),
                "open".into(),
                format!("{base}{path}"),
            ],
            None,
        )
        .await?;
        ensure!(
            open_code == 0,
            "agent-browser open {path} failed: {open_output}"
        );
        let _ = capture_program(
            browser.clone(),
            vec![
                "--session".into(),
                session.clone(),
                "wait".into(),
                "--load".into(),
                "networkidle".into(),
            ],
            None,
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let (snapshot_code, snapshot) = capture_program(
            browser.clone(),
            vec![
                "--session".into(),
                session.clone(),
                "snapshot".into(),
                "-i".into(),
                "-c".into(),
            ],
            None,
        )
        .await?;
        ensure!(
            snapshot_code == 0,
            "agent-browser snapshot {path} failed: {snapshot}"
        );
        let snapshot = snapshot.to_ascii_lowercase();
        ensure!(
            expected
                .split('|')
                .any(|candidate| snapshot.contains(&candidate.to_ascii_lowercase())),
            "agent-browser surface {path} omitted {expected:?}"
        );
        println!("UI surface verified {path}");
    }
    Ok(0)
}

async fn run_bun_script(relative: &str, args: &[&str]) -> anyhow::Result<i32> {
    run_bun_script_with_env(relative, args, &[]).await
}

async fn run_bun_script_with_env(
    relative: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> anyhow::Result<i32> {
    let root = repository_root();
    let (cwd, script) = if relative == "scenarios/c8-emit-js.ts" {
        (root.join("web"), "../scenarios/c8-emit-js.ts".to_owned())
    } else {
        (root.clone(), relative.to_owned())
    };
    let mut command_args = vec![script];
    command_args.extend(args.iter().map(|arg| (*arg).to_owned()));
    run_program_with_env(PathBuf::from("bun"), command_args, Some(cwd), env).await
}

fn synthetic_headers(trace_id: &str, span_id: u64) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        "traceparent",
        format!("00-{trace_id}-{span_id:016x}-01").parse()?,
    );
    headers.insert("tracestate", "playground=commerce".parse()?);
    headers.insert(
        "baggage",
        "tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal".parse()?,
    );
    Ok(headers)
}

async fn synthetic_order_request(
    label: &str,
    trace_id: &str,
    span_id: u64,
    poison: bool,
    orphan: bool,
    lag_ms: u64,
) -> anyhow::Result<Value> {
    let base = url_env("ORDERS_URL", "http://localhost:8092");
    let mut url = Url::parse(&format!("{base}/order"))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("tenant_id", "tenant-acme");
        pairs.append_pair("customer_id", "customer-acme-ava");
        if poison {
            pairs.append_pair("poison", "1");
        }
        if orphan {
            pairs.append_pair("orphan", "1");
        }
        if lag_ms > 0 {
            pairs.append_pair("lag_ms", &lag_ms.to_string());
        }
    }
    let (status, body) = request_json(
        Method::POST,
        url.as_str(),
        synthetic_headers(trace_id, span_id)?,
        None,
    )
    .await?;
    ensure!(
        status.is_success(),
        "{label} synthetic order failed: HTTP {status}: {body}"
    );
    println!("{label}: HTTP {status} {body}");
    Ok(body)
}

async fn synthetic_batch_fanin() -> anyhow::Result<i32> {
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    let mut children = JoinSet::new();
    for span_id in 1..=8 {
        let label = format!("fan-in message {span_id}");
        children.spawn(async move {
            synthetic_order_request(&label, trace_id, span_id, false, false, 100).await
        });
    }
    while let Some(result) = children.join_next().await {
        result??;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    println!("synthetic RabbitMQ fan-in completed across 8 producer messages");
    Ok(0)
}

async fn synthetic_poison_retry() -> anyhow::Result<i32> {
    let trace_id = "0af7651916cd43dd8448eb211c80319c";
    synthetic_order_request("synthetic lag", trace_id, 7, false, false, 300).await?;
    synthetic_order_request("synthetic poison", trace_id, 8, true, false, 0).await?;
    println!("synthetic lag and poison retry workload completed");
    Ok(0)
}

async fn synthetic_orphan_consumer() -> anyhow::Result<i32> {
    let linked_trace = "b7f2d9a4c6e81f03579ab2cd4e6f8102";
    let orphan_trace = "c8e3dab5f7a9201468ab3cd5f7a90213";
    synthetic_order_request(
        "linked synthetic consumer",
        linked_trace,
        21,
        false,
        false,
        0,
    )
    .await?;
    synthetic_order_request(
        "orphan synthetic consumer",
        orphan_trace,
        22,
        false,
        true,
        0,
    )
    .await?;
    let mut children = JoinSet::new();
    for index in 1..=6 {
        let label = format!("synthetic lag {index}");
        children.spawn(async move {
            synthetic_order_request(&label, linked_trace, 100 + index, false, false, 2_000).await
        });
    }
    while let Some(result) = children.join_next().await {
        result??;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!("synthetic linked, orphan, and lag burst completed");
    Ok(0)
}

async fn container_oom(args: &[String]) -> anyhow::Result<i32> {
    ensure!(
        args.iter().any(|arg| arg == "--yes"),
        "container:recommendation_oom_probe is destructive; pass --yes explicitly"
    );
    let root = repository_root();
    let compose = root.join("deploy/docker-compose.yml");
    let limits = root.join("deploy/docker-compose.limits.yml");
    let leak_kb = positive_env("LEAK_KB", 8_192)?;
    let rounds = positive_env("ROUNDS", 32)?;
    let compose_files = vec![
        "compose".to_owned(),
        "-f".to_owned(),
        compose.display().to_string(),
        "-f".to_owned(),
        limits.display().to_string(),
    ];
    let mut start_args = compose_files.clone();
    start_args.extend(["up".into(), "-d".into(), "recommendation".into()]);
    run_program(PathBuf::from("docker"), start_args, Some(root.clone())).await?;

    let base = url_env("RECOMMENDATION_URL", "http://localhost:8090");
    for round in 1..=rounds {
        let url = format!("{base}/recommend?tenant_id=tenant-acme&sku=WIDGET-1&leak={leak_kb}");
        match request_json(Method::GET, &url, HeaderMap::new(), None).await {
            Ok((status, body)) => println!("leak round {round}: {leak_kb}KiB [{status}] {body}"),
            Err(error) => eprintln!("leak round {round}: {leak_kb}KiB request failed: {error}"),
        }

        let mut status_args = compose_files.clone();
        status_args.extend(["ps".into(), "recommendation".into()]);
        run_program(PathBuf::from("docker"), status_args, Some(root.clone())).await?;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    println!(
        "recommendation OOM probe complete: {rounds} rounds at {leak_kb}KiB; inspect restart window in Parallax"
    );
    Ok(0)
}

async fn release_regression() -> anyhow::Result<i32> {
    let root = repository_root();
    let compose_file = root.join("deploy/docker-compose.yml");
    ensure!(
        compose_file.is_file(),
        "A13 Compose file is missing: {}",
        compose_file.display()
    );
    let base = url_env("CHECKOUT_URL", "http://localhost:8088");
    let requests = positive_env("A13_REQUESTS", 5)?;
    let build = std::env::var("A13_BUILD")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let project = std::env::var("A13_COMPOSE_PROJECT")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let lifecycle = release_regression_lifecycle(
        &root,
        &compose_file,
        project.as_deref(),
        &base,
        requests,
        build,
    )
    .await;

    // Always put checkout back on v1 after either phase. This mirrors the
    // script trap while keeping the Compose invocation shell-free.
    println!("A13 cleanup: restoring checkout RELEASE=v1");
    let cleanup = run_compose(
        &root,
        &compose_file,
        project.as_deref(),
        "v1",
        &["up", "-d", "--no-deps", "--force-recreate", "checkout"],
    )
    .await;

    match (lifecycle, cleanup) {
        (Ok(code), Ok(())) => Ok(code),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error.context("A13 cleanup failed while restoring RELEASE=v1")),
        (Err(error), Err(cleanup_error)) => bail!(
            "A13 lifecycle failed: {error}; cleanup failed while restoring RELEASE=v1: {cleanup_error}"
        ),
    }
}

async fn release_regression_lifecycle(
    root: &Path,
    compose_file: &Path,
    project: Option<&str>,
    base: &str,
    requests: u64,
    build: bool,
) -> anyhow::Result<i32> {
    println!("A13 phase 1: checkout RELEASE=v1 baseline");
    let mut start_args = vec!["up", "-d"];
    if build {
        start_args.push("--build");
    }
    start_args.extend([
        "flagd",
        "pricing",
        "inventory",
        "recommendation",
        "checkout",
    ]);
    run_compose(root, compose_file, project, "v1", &start_args).await?;
    wait_for_checkout(base).await?;
    release_checkout_burst(base, "v1-baseline", requests).await?;

    println!("A13 phase 2: checkout RELEASE=v2 release attribution");
    run_compose(
        root,
        compose_file,
        project,
        "v2",
        &["up", "-d", "--no-deps", "--force-recreate", "checkout"],
    )
    .await?;
    wait_for_checkout(base).await?;
    release_checkout_burst(base, "v2-attribution", requests).await?;
    println!("A13 release phases passed; compare service.version=v1 and v2 in Parallax");
    Ok(0)
}

async fn run_compose(
    root: &Path,
    compose_file: &Path,
    project: Option<&str>,
    release: &str,
    command: &[&str],
) -> anyhow::Result<()> {
    let mut args = vec![
        "compose".to_owned(),
        "-f".to_owned(),
        compose_file.display().to_string(),
    ];
    if let Some(project) = project {
        args.extend(["--project-name".to_owned(), project.to_owned()]);
    }
    args.extend(command.iter().map(|arg| (*arg).to_owned()));
    run_program_with_env(
        PathBuf::from("docker"),
        args,
        Some(root.to_owned()),
        &[("RELEASE", release)],
    )
    .await
    .with_context(|| format!("docker Compose RELEASE={release} command failed"))?;
    Ok(())
}

async fn wait_for_checkout(base: &str) -> anyhow::Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to build checkout readiness client")?;
    for _ in 0..30 {
        if let Ok(response) = client.get(format!("{base}/healthz")).send().await
            && response.status().is_success()
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("checkout did not become reachable at {base}")
}

async fn release_checkout_burst(base: &str, label: &str, requests: u64) -> anyhow::Result<()> {
    for index in 1..=requests {
        let request = json!({
            "tenant_id": "tenant-acme",
            "customer_id": "customer-acme-ava",
            "items": [{"sku": "WIDGET-1", "quantity": 1}],
            "currency_code": "USD",
            "payment_method_token": "tok_visa",
            "payment_method_type": "card",
            "request_id": format!("a13-{label}-{index}"),
        });
        let (status, body) = request_json(
            Method::POST,
            &format!("{base}/checkout"),
            json_headers(),
            Some(request),
        )
        .await?;
        ensure!(
            status == StatusCode::OK,
            "{label} #{index}: expected checkout HTTP 200, got {status}: {body}"
        );
        println!("{label} #{index} [{status}]");
    }
    Ok(())
}

async fn corpus_all() -> anyhow::Result<i32> {
    for scenario in [
        "traces:deep",
        "traces:wide",
        "traces:multi_root",
        "traces:orphan",
        "traces:clock_skew",
        "traces:zero_duration",
        "traces:cross_links",
        "traces:long_names",
        "traces:events",
        "logs:burst",
        "logs:bodies",
        "logs:patterns",
        "metrics:shapes",
        "metrics:labels",
        "attributes:bounded",
        "issues:burst",
        "issues:multi_language",
        "protocols:grpc_errors",
        "protocols:grpc_stream",
        "protocols:graphql_errors",
        "protocols:rabbitmq_lag",
        "journeys:happy_path",
        "journeys:error_path",
        "journeys:outside_screen",
        "journeys:reattach",
        "journeys:parallel",
        "ecosystem:external_edge",
        "ecosystem:full",
    ] {
        let code = match scenario {
            "traces:deep" => run_shape("t-deep").await?,
            "traces:wide" => run_shape("t-wide").await?,
            "traces:multi_root" => run_shape("t-multiroot").await?,
            "traces:orphan" => run_shape("t-orphan").await?,
            "traces:clock_skew" => run_shape("t-skew").await?,
            "traces:zero_duration" => run_shape("t-zero").await?,
            "traces:cross_links" => run_shape("t-links").await?,
            "traces:long_names" => run_shape("t-longnames").await?,
            "traces:events" => run_shape("t-events").await?,
            "logs:burst" => run_shape("l-burst").await?,
            "logs:bodies" => run_shape("l-bodies").await?,
            "logs:patterns" => run_shape("l-patterns").await?,
            "metrics:shapes" => run_shape("m-shapes").await?,
            "metrics:labels" => run_shape("m-labels").await?,
            "attributes:bounded" => run_shape("f-attrs").await?,
            "issues:burst" => run_shape("e-burst").await?,
            "issues:multi_language" => run_shape("e-multi-lang").await?,
            "protocols:grpc_errors" => grpc_error_corpus().await?,
            "protocols:grpc_stream" => pricing_stream().await?,
            "protocols:graphql_errors" => graphql_shapes().await?,
            "protocols:rabbitmq_lag" => synthetic_poison_retry().await?,
            "journeys:happy_path" => journey(&["--seconds", "6"]).await?,
            "journeys:error_path" => expected_journey_failure().await?,
            "journeys:outside_screen" => journey(&["--seconds", "6", "--outside-error"]).await?,
            "journeys:reattach" => journey(&["--seconds", "9", "--reattach", "3"]).await?,
            "journeys:parallel" => parallel_journeys().await?,
            "ecosystem:external_edge" => run_shape("eco-external").await?,
            "ecosystem:full" => ecosystem_full().await?,
            _ => unreachable!("corpus registry is fixed"),
        };
        ensure!(code == 0, "corpus member {scenario} failed");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::{SCENARIO_NAMES, hex_id};

    #[test]
    fn semantic_names_are_unique_and_grouped() {
        let mut names = SCENARIO_NAMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SCENARIO_NAMES.len());
        assert!(SCENARIO_NAMES.iter().all(|name| {
            let mut parts = name.split(':');
            let group = parts.next().unwrap_or_default();
            let case = parts.next().unwrap_or_default();
            parts.next().is_none()
                && !group.is_empty()
                && !case.is_empty()
                && group.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && case
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        }));
    }

    #[test]
    fn generated_ids_have_expected_hex_width() {
        assert_eq!(hex_id(16).len(), 32);
        assert_eq!(hex_id(8).len(), 16);
    }
}
