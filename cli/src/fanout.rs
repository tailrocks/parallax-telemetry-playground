//! Fan-out proof (GOAL.md §11): one deterministic `traces:deep` trace must
//! arrive in Parallax, Jaeger, and OpenObserve through the comparison
//! collector (`deploy/comparison/`). This is a standalone lab check,
//! deliberately not a registered scenario: the corpus gate counts proofs
//! exactly, and this lab check must not perturb it.
//!
//! Contract: emit once via the scenario runner pointed at the collector's
//! OTLP/HTTP ingress, then poll all three backends until the full span count
//! of the deterministic trace is queryable in each.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

use crate::shapes;

const POLL_INTERVAL: Duration = Duration::from_secs(3);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const OO_WINDOW_MICROS: i64 = 3_600_000_000;
const OO_PAGE_SIZE: i64 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    parallax_url: String,
    fanout_otlp_http: String,
    jaeger_url: String,
    oo_url: String,
    timeout_secs: u64,
}

impl Config {
    fn defaults() -> Self {
        Self {
            parallax_url: "http://127.0.0.1:4000".to_owned(),
            fanout_otlp_http: "http://127.0.0.1:24318".to_owned(),
            jaeger_url: "http://127.0.0.1:36686".to_owned(),
            oo_url: "http://127.0.0.1:5080".to_owned(),
            timeout_secs: 60,
        }
    }

    fn from_env() -> Self {
        let defaults = Self::defaults();
        Self {
            parallax_url: base_url("PARALLAX_URL", &defaults.parallax_url),
            fanout_otlp_http: base_url("FANOUT_OTLP_HTTP", &defaults.fanout_otlp_http),
            jaeger_url: base_url("JAEGER_URL", &defaults.jaeger_url),
            oo_url: base_url("OO_URL", &defaults.oo_url),
            timeout_secs: std::env::var("FANOUT_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .filter(|secs| *secs > 0)
                .unwrap_or(defaults.timeout_secs),
        }
    }
}

fn base_url(var: &str, default: &str) -> String {
    normalized_base(std::env::var(var).ok().as_deref(), default)
}

/// Endpoint normalization: unset/blank falls back to the default; trailing
/// slashes are stripped so URL joins stay single-slash.
fn normalized_base(value: Option<&str>, default: &str) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/'))
        .map(str::to_owned)
        .unwrap_or_else(|| default.to_owned())
}

pub(crate) async fn run(args: &[String]) -> Result<i32> {
    ensure!(
        args.is_empty(),
        "usage: playground check-fanout (configuration comes from PARALLAX_URL, FANOUT_OTLP_HTTP, JAEGER_URL, OO_URL, FANOUT_TIMEOUT_SECS)"
    );
    let config = Config::from_env();
    let expected_spans = shapes::t_deep().len();
    let trace_id = emit(&config).await?;
    println!("trace: {trace_id}");
    poll_backends(&config, &trace_id, expected_spans).await?;
    println!("fan-out proof complete: same trace in all three backends");
    Ok(0)
}

/// Emit the deterministic trace through the fan-out collector by re-running
/// this executable as the `traces:deep` scenario with the collector pinned as
/// the OTLP endpoint (both standard variables, so an inherited endpoint
/// cannot redirect the emit).
async fn emit(config: &Config) -> Result<String> {
    let executable = std::env::current_exe().context("cannot locate the playground executable")?;
    let output = tokio::process::Command::new(&executable)
        .args(["scenario", "traces:deep"])
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", &config.fanout_otlp_http)
        .env("OTEL_EXPORTER_OTLP_HTTP_ENDPOINT", &config.fanout_otlp_http)
        .output()
        .await
        .context("cannot run the traces:deep emit")?;
    ensure!(
        output.status.success(),
        "traces:deep emit failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trace_id = extract_trace_id(&stdout)
        .with_context(|| format!("emit produced no trace id; stdout: {}", stdout.trim()))?;
    Ok(trace_id.to_owned())
}

fn extract_trace_id(stdout: &str) -> Option<&str> {
    let start = stdout.find("shapes: trace ")? + "shapes: trace ".len();
    let candidate = stdout.get(start..start + 32)?;
    ensure_hex(candidate).then_some(candidate)
}

fn ensure_hex(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct Backend {
    name: &'static str,
    seen: Option<usize>,
    passed: bool,
}

impl Backend {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            seen: None,
            passed: false,
        }
    }
}

async fn poll_backends(config: &Config, trace_id: &str, expected_spans: usize) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("cannot build the fan-out HTTP client")?;
    let mut backends = [
        Backend::new("parallax"),
        Backend::new("jaeger"),
        Backend::new("openobserve"),
    ];
    let deadline = Instant::now() + Duration::from_secs(config.timeout_secs);
    loop {
        let counts = [
            parallax_span_count(&client, &config.parallax_url, trace_id).await,
            jaeger_span_count(&client, &config.jaeger_url, trace_id).await,
            oo_span_count(&client, &config.oo_url, trace_id).await,
        ];
        for (backend, count) in backends.iter_mut().zip(counts) {
            if let Some(count) = count {
                backend.seen = Some(count);
                if !backend.passed && count == expected_spans {
                    backend.passed = true;
                    println!(
                        "PASS {}: {count}/{expected_spans} spans for {trace_id}",
                        backend.name
                    );
                }
            }
        }
        if backends.iter().all(|backend| backend.passed) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let summary = backends
                .iter()
                .map(|backend| {
                    format!(
                        "{}: {}/{}",
                        backend.name,
                        backend
                            .seen
                            .map(|seen| seen.to_string())
                            .unwrap_or_else(|| "none".to_owned()),
                        expected_spans
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            bail!(
                "fan-out incomplete after {}s: {summary}",
                config.timeout_secs
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Span count seen by Parallax GraphQL; `None` keeps polling (transport
/// trouble is not evidence of absence).
async fn parallax_span_count(
    client: &reqwest::Client,
    parallax_url: &str,
    trace_id: &str,
) -> Option<usize> {
    let response = client
        .post(format!("{parallax_url}/graphql"))
        .json(&json!({
            "query": format!(r#"{{ trace(traceId: {trace_id:?}) {{ spans {{ spanId }} }} }}"#)
        }))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    Some(parallax_spans_in_response(&response))
}

/// Counted spans in a Parallax GraphQL response; anything that is not an
/// indexed trace counts as zero.
fn parallax_spans_in_response(response: &Value) -> usize {
    response
        .pointer("/data/trace/spans")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

/// Span count seen by Jaeger; `None` keeps polling.
async fn jaeger_span_count(
    client: &reqwest::Client,
    jaeger_url: &str,
    trace_id: &str,
) -> Option<usize> {
    let response = client
        .get(format!("{jaeger_url}/api/traces/{trace_id}"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    Some(jaeger_spans_in_response(&response))
}

/// Counted spans in a Jaeger search response; only an exactly-one-trace
/// payload counts (multiple traces would mean an ambiguous query).
fn jaeger_spans_in_response(response: &Value) -> usize {
    match response.get("data").and_then(Value::as_array) {
        Some(traces) if traces.len() == 1 => traces[0]
            .get("spans")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        _ => 0,
    }
}

/// Span count seen by OpenObserve; `None` keeps polling.
async fn oo_span_count(client: &reqwest::Client, oo_url: &str, trace_id: &str) -> Option<usize> {
    let now = oo_now_micros();
    let response = client
        .post(format!("{oo_url}/api/default/_search?type=traces"))
        .basic_auth("root@example.com", Some("Complexpass#123"))
        .json(&oo_search_body(trace_id, now))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    Some(oo_hits_in_response(&response))
}

fn oo_now_micros() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_micros() as i64)
        .unwrap_or_default()
}

fn oo_search_body(trace_id: &str, now_micros: i64) -> Value {
    json!({
        "query": {
            "sql": format!(r#"SELECT trace_id, span_id FROM "default" WHERE trace_id = '{trace_id}'"#),
            "start_time": now_micros - OO_WINDOW_MICROS,
            "end_time": now_micros,
            "size": OO_PAGE_SIZE,
        }
    })
}

fn oo_hits_in_response(response: &Value) -> usize {
    response
        .get("hits")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn emit_stdout() -> String {
        "shapes: t-deep emitted (invocation inv-1)\nshapes: trace 0123456789abcdef0123456789abcdef\n"
            .to_owned()
    }

    #[test]
    fn trace_id_is_extracted_from_the_emit_stdout() {
        assert_eq!(
            extract_trace_id(&emit_stdout()),
            Some("0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn missing_or_corrupt_trace_id_keeps_the_proof_failing() {
        assert_eq!(extract_trace_id("shapes: trace nothex"), None);
        assert_eq!(extract_trace_id("shapes: trace 0123456789abcdefzzzz"), None);
        assert_eq!(extract_trace_id(""), None);
    }

    #[test]
    fn parallax_span_count_reads_the_trace_spans_array() {
        let indexed = json!({"data": {"trace": {"spans": [
            {"spanId": "1"}, {"spanId": "2"}, {"spanId": "3"}
        ]}}});
        assert_eq!(parallax_spans_in_response(&indexed), 3);
        assert_eq!(
            parallax_spans_in_response(&json!({"data": {"trace": null}})),
            0
        );
        assert_eq!(parallax_spans_in_response(&json!({})), 0);
        assert_eq!(
            parallax_spans_in_response(&json!({"errors": [{"message": "x"}]})),
            0
        );
    }

    #[test]
    fn jaeger_span_count_requires_exactly_one_trace() {
        let indexed = json!({"data": [{"spans": [{"spanID": "1"}, {"spanID": "2"}]}]});
        assert_eq!(jaeger_spans_in_response(&indexed), 2);
        assert_eq!(jaeger_spans_in_response(&json!({"data": []})), 0);
        assert_eq!(jaeger_spans_in_response(&json!({"data": [{}, {}]})), 0);
        assert_eq!(jaeger_spans_in_response(&json!({})), 0);
    }

    #[test]
    fn oo_span_count_reads_search_hits() {
        let indexed = json!({"hits": [{"trace_id": "t"}, {"trace_id": "t"}]});
        assert_eq!(oo_hits_in_response(&indexed), 2);
        assert_eq!(oo_hits_in_response(&json!({"hits": []})), 0);
        assert_eq!(oo_hits_in_response(&json!({})), 0);
    }

    #[test]
    fn oo_search_query_scopes_the_trace_and_window() {
        let body = oo_search_body("abc", 1_000_000_000_000);
        let query = &body["query"];
        assert!(
            query["sql"]
                .as_str()
                .is_some_and(|sql| sql.contains(r#"FROM "default""#) && sql.contains("abc"))
        );
        assert_eq!(
            query["start_time"],
            1_000_000_000_000_i64 - OO_WINDOW_MICROS
        );
        assert_eq!(query["end_time"], 1_000_000_000_000_i64);
        assert_eq!(query["size"], OO_PAGE_SIZE);
    }

    #[test]
    fn defaults_match_the_comparison_compose_ports() {
        assert_eq!(
            Config::defaults(),
            Config {
                parallax_url: "http://127.0.0.1:4000".to_owned(),
                fanout_otlp_http: "http://127.0.0.1:24318".to_owned(),
                jaeger_url: "http://127.0.0.1:36686".to_owned(),
                oo_url: "http://127.0.0.1:5080".to_owned(),
                timeout_secs: 60,
            }
        );
    }

    #[test]
    fn endpoint_normalization_strips_slashes_and_ignores_blank() {
        assert_eq!(
            normalized_base(Some("http://127.0.0.1:5080///"), "http://x"),
            "http://127.0.0.1:5080"
        );
        assert_eq!(normalized_base(Some("   "), "http://x"), "http://x");
        assert_eq!(normalized_base(None, "http://x"), "http://x");
    }

    #[test]
    fn expected_span_count_is_the_deterministic_deep_trace() {
        assert_eq!(shapes::t_deep().len(), 14);
        let mut span_ids = shapes::t_deep()
            .iter()
            .map(|span| span.id.clone())
            .collect::<Vec<_>>();
        span_ids.sort();
        span_ids.dedup();
        assert_eq!(span_ids.len(), 14, "deep-trace span ids must stay distinct");
    }
}
