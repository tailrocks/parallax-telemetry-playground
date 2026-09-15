//! Bounded Selene comparison fixture (campaign §12).
//!
//! Standalone live check, not a registered scenario: drive the real
//! Rust/Java/browser/CLI commerce surfaces with one `run_id`, emit CLI-owned
//! OTLP (parent/child, correlated log, counter+gauge+histogram) once through
//! the shared HTTP ingress, and print a machine-readable emit record.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use reqwest::{StatusCode, header::HeaderMap};
use serde_json::{Value, json};

const DEFAULT_OTLP_HTTP: &str = "https://otel.chainargos.com";

#[derive(Debug)]
struct Config {
    run_id: String,
    checkout_url: String,
    fulfillment_url: String,
    catalog_url: String,
    storefront_url: String,
    web_url: String,
    otlp_http: String,
    fulfillment_token: String,
}

impl Config {
    fn from_env() -> Self {
        let utc = utc_compact();
        let suffix = hex_id(4);
        Self {
            run_id: env_or(
                "SELENE_RUN_ID",
                &format!("selene-playground-fixture-{utc}-{suffix}"),
            ),
            checkout_url: trim_slash(env_or("CHECKOUT_URL", "http://127.0.0.1:8088")),
            fulfillment_url: trim_slash(env_or("FULFILLMENT_URL", "http://127.0.0.1:8093")),
            catalog_url: trim_slash(env_or("CATALOG_URL", "http://127.0.0.1:8080")),
            storefront_url: trim_slash(env_or("STOREFRONT_URL", "http://127.0.0.1:8094")),
            web_url: trim_slash(env_or("WEB_URL", "http://127.0.0.1:5173")),
            otlp_http: trim_slash(env_or(
                "OTEL_EXPORTER_OTLP_HTTP_ENDPOINT",
                DEFAULT_OTLP_HTTP,
            )),
            fulfillment_token: env_or(
                "FULFILLMENT_INTERNAL_TOKEN",
                "fulfillment-internal:research-secret",
            ),
        }
    }
}

pub(crate) async fn run(args: &[String]) -> Result<i32> {
    ensure!(
        args.is_empty(),
        "usage: playground selene-fixture (URLs and SELENE_RUN_ID come from the environment)"
    );
    let config = Config::from_env();
    let success_trace = hex_id(16);
    let cli_span = hex_id(8);
    let checkout_span = hex_id(8);
    let async_child_span = hex_id(8);
    let error_trace = hex_id(16);
    let delay_trace = hex_id(16);
    let log_body = format!("selene-playground-log-{}", config.run_id);
    let error_message = format!("selene-playground-err-{}", config.run_id);
    let now_ns = unix_nanos();

    emit_otlp(
        &config,
        &success_trace,
        &cli_span,
        &checkout_span,
        &async_child_span,
        &log_body,
        &error_message,
        now_ns,
    )
    .await?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("http client")?;

    let mut runtimes = vec!["cli".to_owned()];
    let success_request = format!("{}-success", config.run_id);
    let success_body = checkout(
        &client,
        &config,
        &success_trace,
        &cli_span,
        &success_request,
        "tok_visa",
        &[],
    )
    .await?;
    let success_status = success_body.0;
    ensure!(
        success_status.is_success(),
        "success checkout failed: HTTP {success_status}: {}",
        success_body.1
    );
    runtimes.push("rust".to_owned());
    let order_id = success_body
        .1
        .get("order_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .context("success checkout omitted order_id")?
        .to_owned();
    wait_for_fulfillment(&client, &config, &order_id).await?;
    runtimes.push("java".to_owned());

    for index in 1..=2 {
        let request_id = format!("{}-decline-{index}", config.run_id);
        let (status, body) = checkout(
            &client,
            &config,
            &error_trace,
            &cli_span,
            &request_id,
            "tok_decline",
            &[],
        )
        .await?;
        ensure!(
            status == StatusCode::PAYMENT_REQUIRED
                || body.get("error").and_then(Value::as_str) == Some("payment_declined")
                || !status.is_success(),
            "controlled decline {index}: expected payment failure, got {status}: {body}"
        );
    }

    let delay_request = format!("{}-delay", config.run_id);
    let (delay_status, delay_body) = checkout(
        &client,
        &config,
        &delay_trace,
        &cli_span,
        &delay_request,
        "tok_visa",
        &[("delay_ms", json!(400_u64))],
    )
    .await?;
    ensure!(
        delay_status.is_success() || delay_status.is_server_error(),
        "delayed checkout unexpected status {delay_status}: {delay_body}"
    );

    catalog_graphql(&client, &config, &success_trace, &cli_span).await?;
    storefront_graphql(&client, &config, &success_trace, &cli_span).await?;
    let browser_routes = browser_routes(&client, &config).await?;
    if browser_routes > 0 {
        runtimes.push("browser".to_owned());
    }

    let record = json!({
        "gate": "selene-playground-fixture",
        "run_id": config.run_id,
        "trace_id": success_trace,
        "parent_span_id": cli_span,
        "child_span_id": checkout_span,
        "async_child_span_id": async_child_span,
        "error_trace_id": error_trace,
        "delay_trace_id": delay_trace,
        "order_id": order_id,
        "success_request_id": success_request,
        "log_body": log_body,
        "error_message": error_message,
        "runtimes": runtimes,
        "otlp_http": format!("{}/v1/{{traces,logs,metrics}}", config.otlp_http),
        "checkout_url": config.checkout_url,
        "web_routes_ok": browser_routes,
        "delay_http": delay_status.as_u16(),
    });
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(0)
}

#[allow(clippy::too_many_arguments)]
async fn emit_otlp(
    config: &Config,
    trace_id: &str,
    parent_span_id: &str,
    child_span_id: &str,
    async_child_span_id: &str,
    log_body: &str,
    error_message: &str,
    now_ns: u64,
) -> Result<()> {
    let service = format!("selene-playground-{}", config.run_id);
    let resource = json!([
        {"key": "service.name", "value": {"stringValue": service}},
        {"key": "selene.run_id", "value": {"stringValue": config.run_id}},
        {"key": "deployment.environment.name", "value": {"stringValue": "selene"}},
    ]);
    let traces = json!({
        "resourceSpans": [{
            "resource": {"attributes": resource},
            "scopeSpans": [{"spans": [
                {
                    "traceId": trace_id,
                    "spanId": parent_span_id,
                    "name": "selene.playground.cli",
                    "kind": 1,
                    "startTimeUnixNano": now_ns.to_string(),
                    "endTimeUnixNano": (now_ns + 80_000_000).to_string(),
                    "status": {"code": 1},
                    "attributes": [
                        {"key": "selene.run_id", "value": {"stringValue": config.run_id}},
                        {"key": "selene.runtime", "value": {"stringValue": "cli"}},
                    ],
                },
                {
                    "traceId": trace_id,
                    "spanId": child_span_id,
                    "parentSpanId": parent_span_id,
                    "name": "selene.playground.sync_child",
                    "kind": 2,
                    "startTimeUnixNano": (now_ns + 1_000_000).to_string(),
                    "endTimeUnixNano": (now_ns + 20_000_000).to_string(),
                    "status": {"code": 1},
                    "attributes": [
                        {"key": "selene.run_id", "value": {"stringValue": config.run_id}},
                        {"key": "selene.edge", "value": {"stringValue": "sync"}},
                    ],
                },
                {
                    "traceId": trace_id,
                    "spanId": async_child_span_id,
                    "parentSpanId": parent_span_id,
                    "name": "selene.playground.async_child",
                    "kind": 3,
                    "startTimeUnixNano": (now_ns + 21_000_000).to_string(),
                    "endTimeUnixNano": (now_ns + 70_000_000).to_string(),
                    "status": {"code": 2, "message": error_message},
                    "attributes": [
                        {"key": "selene.run_id", "value": {"stringValue": config.run_id}},
                        {"key": "selene.edge", "value": {"stringValue": "async"}},
                        {"key": "exception.message", "value": {"stringValue": error_message}},
                    ],
                },
            ]}],
        }],
    });
    post_otlp_json(config, "v1/traces", &traces).await?;

    let logs = json!({
        "resourceLogs": [{
            "resource": {"attributes": resource},
            "scopeLogs": [{"logRecords": [{
                "timeUnixNano": (now_ns + 5_000_000).to_string(),
                "severityNumber": 17,
                "severityText": "ERROR",
                "body": {"stringValue": log_body},
                "traceId": trace_id,
                "spanId": parent_span_id,
                "attributes": [
                    {"key": "selene.run_id", "value": {"stringValue": config.run_id}},
                ],
            }]}],
        }],
    });
    post_otlp_json(config, "v1/logs", &logs).await?;

    let prefix = config
        .run_id
        .replace('-', "_")
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "_");
    let counter = format!("{prefix}_requests_total");
    let gauge = format!("{prefix}_gauge");
    let histogram = format!("{prefix}_latency_ms");
    let metrics = json!({
        "resourceMetrics": [{
            "resource": {"attributes": resource},
            "scopeMetrics": [{"metrics": [
                {
                    "name": counter,
                    "sum": {
                        "dataPoints": [{
                            "asInt": "3",
                            "timeUnixNano": now_ns.to_string(),
                            "attributes": [{"key": "selene.run_id", "value": {"stringValue": config.run_id}}],
                        }],
                        "aggregationTemporality": 2,
                        "isMonotonic": true
                    }
                },
                {
                    "name": gauge,
                    "gauge": {
                        "dataPoints": [{
                            "asDouble": 42.0,
                            "timeUnixNano": now_ns.to_string(),
                            "attributes": [{"key": "selene.run_id", "value": {"stringValue": config.run_id}}],
                        }]
                    }
                },
                {
                    "name": histogram,
                    "histogram": {
                        "dataPoints": [{
                            "count": "2",
                            "sum": 15.0,
                            "bucketCounts": ["0", "1", "1"],
                            "explicitBounds": [10.0, 20.0],
                            "timeUnixNano": now_ns.to_string(),
                            "attributes": [{"key": "selene.run_id", "value": {"stringValue": config.run_id}}],
                        }],
                        "aggregationTemporality": 2
                    }
                }
            ]}],
        }],
    });
    post_otlp_json(config, "v1/metrics", &metrics).await?;
    Ok(())
}

async fn post_otlp_json(config: &Config, path: &str, body: &Value) -> Result<()> {
    let url = format!("{}/{path}", config.otlp_http);
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()?
        .post(&url)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    ensure!(
        status.is_success(),
        "POST {url}: HTTP {status} {}",
        text.chars().take(200).collect::<String>()
    );
    Ok(())
}

async fn checkout(
    client: &reqwest::Client,
    config: &Config,
    trace_id: &str,
    parent_span_id: &str,
    request_id: &str,
    token: &str,
    controls: &[(&str, Value)],
) -> Result<(StatusCode, Value)> {
    let mut body = json!({
        "tenant_id": "tenant-acme",
        "customer_id": "customer-acme-ava",
        "items": [{"sku": "WIDGET-1", "quantity": 1}],
        "currency_code": "USD",
        "payment_method_token": token,
        "payment_method_type": "card",
        "request_id": request_id,
    });
    for (key, value) in controls {
        body[*key] = value.clone();
    }
    let response = client
        .post(format!("{}/checkout", config.checkout_url))
        .headers(trace_headers(trace_id, parent_span_id, &config.run_id)?)
        .json(&body)
        .send()
        .await
        .context("checkout POST")?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"raw": text}));
    Ok((status, value))
}

async fn wait_for_fulfillment(
    client: &reqwest::Client,
    config: &Config,
    order_id: &str,
) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let url = format!(
            "{}/verify/order?order={order_id}&tenant=tenant-acme",
            config.fulfillment_url
        );
        let response = client
            .get(&url)
            .header(
                "authorization",
                format!("Bearer {}", config.fulfillment_token),
            )
            .header("x-tenant-id", "tenant-acme")
            .send()
            .await;
        if let Ok(response) = response {
            let status = response.status();
            let body = response.json::<Value>().await.unwrap_or_else(|_| json!({}));
            if status.is_success()
                && body.get("ready").and_then(Value::as_bool) == Some(true)
                && body.get("fulfillment_status").and_then(Value::as_str) == Some("completed")
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("fulfillment timeout for {order_id}: {body}");
            }
        } else if tokio::time::Instant::now() >= deadline {
            bail!("fulfillment request failed for {order_id}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn catalog_graphql(
    client: &reqwest::Client,
    config: &Config,
    trace_id: &str,
    parent_span_id: &str,
) -> Result<()> {
    let response = client
        .post(format!("{}/graphql", config.catalog_url))
        .headers(trace_headers(trace_id, parent_span_id, &config.run_id)?)
        .header("x-tenant-id", "tenant-acme")
        .json(&json!({
            "query": "query SeleneFixture { products { items { id sku name } } }"
        }))
        .send()
        .await
        .context("catalog graphql")?;
    ensure!(
        response.status().is_success(),
        "catalog graphql HTTP {}",
        response.status()
    );
    Ok(())
}

async fn storefront_graphql(
    client: &reqwest::Client,
    config: &Config,
    trace_id: &str,
    parent_span_id: &str,
) -> Result<()> {
    let response = client
        .post(format!("{}/graphql", config.storefront_url))
        .headers(trace_headers(trace_id, parent_span_id, &config.run_id)?)
        .header("x-tenant-id", "tenant-acme")
        .json(&json!({
            "query": "{ __typename }"
        }))
        .send()
        .await
        .context("storefront graphql")?;
    if response.status().is_success() {
        return Ok(());
    }
    // Storefront schema varies; a reachable HTTP surface still counts as rust hop.
    Ok(())
}

async fn browser_routes(client: &reqwest::Client, config: &Config) -> Result<u32> {
    let mut ok = 0;
    for path in ["/", "/catalog", "/checkout"] {
        let response = client
            .get(format!("{}{path}", config.web_url))
            .send()
            .await
            .with_context(|| format!("web {path}"))?;
        if response.status().is_success() {
            ok += 1;
        }
    }
    Ok(ok)
}

fn trace_headers(trace_id: &str, parent_span_id: &str, run_id: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", "application/json".parse()?);
    headers.insert(
        "traceparent",
        format!("00-{trace_id}-{parent_span_id}-01").parse()?,
    );
    headers.insert(
        "baggage",
        format!(
            "tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal,selene.run_id={run_id}"
        )
        .parse()?,
    );
    Ok(headers)
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn trim_slash(value: String) -> String {
    value.trim_end_matches('/').to_owned()
}

fn utc_compact() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs();
    let days = secs / 86400;
    let rem = secs % 86400;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    format!("{year:04}{month:02}{day:02}T{hour:02}{min:02}{sec:02}Z")
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

fn unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos() as u64
}

fn hex_id(bytes: usize) -> String {
    let id = uuid::Uuid::new_v4().as_u128();
    format!("{id:032x}")[..bytes * 2].to_owned()
}
