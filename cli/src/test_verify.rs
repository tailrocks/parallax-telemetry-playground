use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail, ensure};
use playground_telemetry::semconv;
use serde_json::{Value, json};

const POLL_ATTEMPTS: usize = 15;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) traces: usize,
    pub(crate) test_attempts: usize,
    pub(crate) app_descendants: usize,
}

pub(crate) async fn verify(api_url: &str, invocation_id: &str, stack: &str) -> Result<Summary> {
    ensure!(
        matches!(stack, "rust" | "java" | "web"),
        "unknown stack `{stack}`"
    );
    let endpoint = format!("{}/graphql", api_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let mut last_error = None;
    for attempt in 1..=POLL_ATTEMPTS {
        match fetch_and_analyze(&client, &endpoint, invocation_id, stack).await {
            Ok(summary) => return Ok(summary),
            Err(error) => {
                last_error = Some(error);
                if attempt != POLL_ATTEMPTS {
                    eprintln!(
                        "waiting for Parallax to index observable invocation {invocation_id} ({attempt}/{POLL_ATTEMPTS})"
                    );
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
        }
    }
    Err(last_error.expect("poll loop always records an error"))
}

async fn fetch_and_analyze(
    client: &reqwest::Client,
    endpoint: &str,
    invocation_id: &str,
    stack: &str,
) -> Result<Summary> {
    let overview = graphql(
        client,
        endpoint,
        &format!(
            r#"{{ invocation(invocationId: {invocation_id:?}) {{ status exitCode }} tracesByInvocation(invocationId: {invocation_id:?}, limit: 100) {{ traceId }} }}"#
        ),
    )
    .await?;
    let invocation = overview
        .pointer("/data/invocation")
        .context("invocation is not indexed yet")?;
    ensure!(
        invocation["status"] == "finished",
        "invocation has not finished yet"
    );
    ensure!(
        invocation["exitCode"].as_i64() == Some(0),
        "observable runner did not finish successfully"
    );
    let trace_ids = overview
        .pointer("/data/tracesByInvocation")
        .and_then(Value::as_array)
        .context("invocation traces are not indexed yet")?
        .iter()
        .filter_map(|trace| trace["traceId"].as_str())
        .collect::<Vec<_>>();
    ensure!(!trace_ids.is_empty(), "invocation has no indexed traces");

    let mut traces = Vec::with_capacity(trace_ids.len());
    for trace_id in &trace_ids {
        let response = graphql(
            client,
            endpoint,
            &format!(
                r#"{{ trace(traceId: {trace_id:?}) {{ spans {{ spanId parentSpanId name statusCode attributes resource }} }} }}"#
            ),
        )
        .await?;
        traces.push(response);
    }
    analyze(invocation_id, stack, &traces)
}

async fn graphql(client: &reqwest::Client, endpoint: &str, query: &str) -> Result<Value> {
    let mut request = client.post(endpoint).json(&json!({"query": query}));
    if let Ok(token) = std::env::var("PARALLAX_API_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("cannot reach Parallax GraphQL at {endpoint}"))?
        .error_for_status()?
        .json::<Value>()
        .await?;
    if let Some(errors) = response
        .get("errors")
        .filter(|errors| !errors.as_array().is_none_or(Vec::is_empty))
    {
        bail!("Parallax GraphQL error: {errors}");
    }
    Ok(response)
}

fn analyze(invocation_id: &str, stack: &str, traces: &[Value]) -> Result<Summary> {
    let spans = traces
        .iter()
        .filter_map(|trace| trace.pointer("/data/trace/spans").and_then(Value::as_array))
        .flatten()
        .map(DecodedSpan::try_from)
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        spans.iter().any(|span| span.name == "cli.command"),
        "exported cli.command wrapper parent is missing"
    );
    let tests = spans
        .iter()
        .filter(|span| span.attributes.contains_key(semconv::TEST_CASE_NAME))
        .collect::<Vec<_>>();
    ensure!(!tests.is_empty(), "no test-attempt spans were indexed");
    ensure!(
        tests.iter().all(|span| {
            attribute_string(&span.attributes, semconv::CLI_INVOCATION_ID) == Some(invocation_id)
                || attribute_string(&span.resource, semconv::CLI_INVOCATION_ID)
                    == Some(invocation_id)
        }),
        "one or more test spans lost the invocation identity"
    );
    ensure!(
        tests
            .iter()
            .any(|span| attribute_string(&span.attributes, semconv::PARALLAX_TEST_ID).is_some()),
        "explicit test identity fixture is missing"
    );
    ensure!(
        tests.iter().any(
            |span| attribute_string(&span.attributes, semconv::TEST_CASE_PARAMETERS).is_some()
        ),
        "parameterized-test fixture is missing"
    );
    ensure!(
        tests.iter().any(|span| {
            [
                semconv::TEST_CONFIGURATION_OS,
                semconv::TEST_CONFIGURATION_ENVIRONMENT,
                semconv::TEST_CONFIGURATION_BROWSER,
            ]
            .iter()
            .any(|key| span.attributes.contains_key(*key))
        }),
        "test configuration attributes are missing"
    );
    ensure!(
        tests.iter().any(
            |span| attribute_i64(&span.attributes, semconv::TEST_ATTEMPT_ORDINAL)
                .is_some_and(|ordinal| ordinal > 1)
        ),
        "retry attempt evidence is missing"
    );
    ensure!(
        tests.iter().any(|span| attribute_string(
            &span.attributes,
            semconv::TEST_CASE_RESULT_STATUS
        ) == Some(semconv::TEST_RESULT_STATUS_PASS)),
        "passing test evidence is missing"
    );
    ensure!(
        tests.iter().any(|span| attribute_string(
            &span.attributes,
            semconv::TEST_CASE_FAILURE_KIND
        ) == Some(semconv::TEST_FAILURE_KIND_ASSERTION)),
        "assertion-failure evidence is missing"
    );
    ensure!(
        tests.iter().any(|span| attribute_string(
            &span.attributes,
            semconv::TEST_CASE_FAILURE_KIND
        ) == Some(semconv::TEST_FAILURE_KIND_HARNESS)),
        "harness-error evidence is missing"
    );
    ensure!(
        tests
            .iter()
            .all(|span| attribute_string(&span.resource, semconv::VCS_REF_HEAD_REVISION).is_some()),
        "test resource revision is missing"
    );
    ensure!(
        tests
            .iter()
            .all(|span| attribute_string(&span.resource, semconv::SERVICE_VERSION).is_some()),
        "test resource service version is missing"
    );
    ensure!(
        tests.iter().any(|span| span.status_code == "ERROR"),
        "failed test span status is not ERROR"
    );

    let by_parent = spans.iter().fold(
        HashMap::<&str, Vec<&DecodedSpan>>::new(),
        |mut map, span| {
            if let Some(parent) = span.parent_span_id.as_deref() {
                map.entry(parent).or_default().push(span);
            }
            map
        },
    );
    let test_ids = tests
        .iter()
        .map(|span| span.span_id.as_str())
        .collect::<HashSet<_>>();
    let mut descendants = 0;
    let mut frontier = test_ids.iter().copied().collect::<Vec<_>>();
    let mut seen = HashSet::new();
    while let Some(parent) = frontier.pop() {
        if !seen.insert(parent) {
            continue;
        }
        for child in by_parent.get(parent).into_iter().flatten() {
            frontier.push(child.span_id.as_str());
            if !child.attributes.contains_key(semconv::TEST_CASE_NAME) {
                descendants += 1;
            }
        }
    }
    ensure!(
        descendants > 0,
        "{stack} test spans have no stitched application descendants"
    );
    Ok(Summary {
        traces: traces.len(),
        test_attempts: tests.len(),
        app_descendants: descendants,
    })
}

#[derive(Debug)]
struct DecodedSpan {
    span_id: String,
    parent_span_id: Option<String>,
    name: String,
    status_code: String,
    attributes: serde_json::Map<String, Value>,
    resource: serde_json::Map<String, Value>,
}

impl TryFrom<&Value> for DecodedSpan {
    type Error = anyhow::Error;

    fn try_from(value: &Value) -> Result<Self> {
        Ok(Self {
            span_id: value["spanId"]
                .as_str()
                .context("spanId is missing")?
                .to_string(),
            parent_span_id: value["parentSpanId"].as_str().map(str::to_string),
            name: value["name"]
                .as_str()
                .context("span name is missing")?
                .to_string(),
            status_code: value["statusCode"].as_str().unwrap_or_default().to_string(),
            attributes: decode_map(&value["attributes"], "attributes")?,
            resource: decode_map(&value["resource"], "resource")?,
        })
    }
}

fn decode_map(value: &Value, field: &str) -> Result<serde_json::Map<String, Value>> {
    serde_json::from_str::<Value>(value.as_str().context(format!("span {field} is missing"))?)?
        .as_object()
        .cloned()
        .context(format!("span {field} is not an object"))
}

fn attribute_string<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

fn attribute_i64(map: &serde_json::Map<String, Value>, key: &str) -> Option<i64> {
    map.get(key).and_then(Value::as_i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_fixture() -> Value {
        let run = "run-1";
        let resource =
            json!({"cli.invocation.id": run, "vcs.ref.head.revision": "abc", "service.version": "1"})
                .to_string();
        let span = |id: &str, parent: Option<&str>, name: &str, status: &str, attributes: Value| {
            json!({
                "spanId": id, "parentSpanId": parent, "name": name, "statusCode": status,
                "attributes": attributes.to_string(), "resource": resource
            })
        };
        json!({"data":{"trace":{"spans":[
            span("root", None, "cli.command", "UNSET", json!({})),
            span("fail", Some("root"), "test.case", "ERROR", json!({"test.case.name":"assert", "cli.invocation.id":run, "parallax.test.id":"explicit", "test.case.parameters":"x=1", "test.configuration.os":"linux", "test.attempt.ordinal":1, "test.case.result.status":"fail", "test.case.failure.kind":"assertion_failure"})),
            span("error", Some("root"), "test.case", "ERROR", json!({"test.case.name":"harness", "cli.invocation.id":run, "test.configuration.os":"linux", "test.attempt.ordinal":1, "test.case.result.status":"fail", "test.case.failure.kind":"harness_error"})),
            span("pass", Some("root"), "test.case", "UNSET", json!({"test.case.name":"assert", "cli.invocation.id":run, "test.case.parameters":"x=1", "test.configuration.os":"linux", "test.attempt.ordinal":2, "test.case.result.status":"pass"})),
            span("app", Some("pass"), "http.client", "UNSET", json!({}))
        ]}}})
    }

    #[test]
    fn acceptance_contract_requires_complete_payload_and_stitching() -> Result<()> {
        let fixture = complete_fixture();
        assert_eq!(
            analyze("run-1", "rust", &[fixture])?,
            Summary {
                traces: 1,
                test_attempts: 3,
                app_descendants: 1
            }
        );
        Ok(())
    }

    #[test]
    fn acceptance_contract_fails_closed_for_each_required_payload_class() -> Result<()> {
        for key in [
            "parallax.test.id",
            "test.case.parameters",
            "test.configuration.os",
            "test.case.failure.kind",
        ] {
            let mut fixture = complete_fixture();
            let spans = fixture
                .pointer_mut("/data/trace/spans")
                .and_then(Value::as_array_mut)
                .context("fixture spans")?;
            for span in spans {
                let Some(raw) = span["attributes"].as_str() else {
                    continue;
                };
                let mut attributes = serde_json::from_str::<Value>(raw)?;
                if let Some(map) = attributes.as_object_mut() {
                    map.remove(key);
                }
                span["attributes"] = Value::String(attributes.to_string());
            }
            assert!(
                analyze("run-1", "web", &[fixture]).is_err(),
                "removing {key} unexpectedly passed"
            );
        }

        let mut unstitched = complete_fixture();
        unstitched
            .pointer_mut("/data/trace/spans")
            .and_then(Value::as_array_mut)
            .context("fixture spans")?
            .retain(|span| span["spanId"] != "app");
        assert!(analyze("run-1", "web", &[unstitched]).is_err());
        Ok(())
    }
}
