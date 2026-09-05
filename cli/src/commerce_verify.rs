//! Parallax-facing assertions for one real commerce checkout trace.
//!
//! The verifier follows OpenTelemetry span links because RabbitMQ consumers
//! may start a new trace. It asserts concrete service, messaging, and
//! ClickHouse-backed analytics evidence rather than accepting names that only
//! look related to the commerce journey.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

const MAX_TRACES: usize = 32;
const MAX_SPANS_PER_TRACE: usize = 4_096;
const MAX_TOTAL_SPANS: usize = 8_192;
const MAX_TYPED_LINKS_PER_SPAN: usize = 128;
const MAX_TOTAL_TYPED_LINKS: usize = 8_192;
const MAX_LINK_ATTRIBUTES_BYTES: usize = 16 * 1024;
const MAX_ATTRIBUTE_JSON_BYTES: usize = 64 * 1024;
const MAX_ERROR_OUTPUT_CHARS: usize = 512;
const MAX_TRACESTATE_HEADER_BYTES: usize = 512;
const MAX_TRACESTATE_MEMBERS: usize = 32;
const MAX_TRACESTATE_MEMBER_BYTES: usize = 256;
const MAX_TRACESTATE_VALUE_BYTES: usize = 256;
const DEFAULT_VERIFY_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_POLL_INTERVAL_MILLIS: u64 = 1_000;
const MAX_VERIFY_TIMEOUT_SECONDS: u64 = 300;
const MAX_POLL_INTERVAL_MILLIS: u64 = 10_000;
const DEFAULT_STOREFRONT_GRAPHQL_URL: &str = "http://127.0.0.1:8094/graphql";
const DEFAULT_TENANT_ID: &str = "tenant-acme";
const REQUIRED_BUSINESS_BAGGAGE_KEYS: [&str; 5] = [
    "tenant.id",
    "user.tier",
    "customer.segment",
    "region",
    "request.priority",
];
const OPTIONAL_BUSINESS_BAGGAGE_KEYS: [&str; 1] = ["session.id"];

const ANALYTICS_QUERY: &str = r#"
    query CommerceVerifyAnalytics($tenantId: String, $eventName: String) {
      analyticsEvents(tenantId: $tenantId, eventName: $eventName, limit: 100) {
        eventId
        tenantId
        eventKey
        eventName
        source
        entityType
        entityId
        traceId
        spanId
        traceparent
        tracestate
        baggage
        properties
        context
      }
    }
"#;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) traces: usize,
    pub(crate) spans: usize,
    pub(crate) services: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BusinessBaggage {
    values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommerceIdentity {
    tenant_id: String,
    event_id: String,
    event_type: String,
    event_key: String,
    order_id: String,
}

#[derive(Debug)]
struct Analysis {
    summary: Summary,
    business_baggage: BusinessBaggage,
    identity: CommerceIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct W3cCarrier {
    traceparent: String,
    tracestate: BTreeMap<String, String>,
    baggage: BTreeMap<String, String>,
}

impl BusinessBaggage {
    #[cfg(test)]
    fn from_entries(entries: &BTreeMap<String, String>, label: &str) -> Result<Self> {
        let mut values = BTreeMap::new();
        for key in REQUIRED_BUSINESS_BAGGAGE_KEYS
            .into_iter()
            .chain(OPTIONAL_BUSINESS_BAGGAGE_KEYS)
        {
            if let Some(value) = entries.get(key) {
                ensure!(
                    !value.trim().is_empty(),
                    "{label} has an empty `{key}` value"
                );
                values.insert(key.to_owned(), value.clone());
            } else if REQUIRED_BUSINESS_BAGGAGE_KEYS.contains(&key) {
                bail!("{label} is missing `{key}`");
            }
        }
        Ok(Self { values })
    }

    fn from_attribute_object(
        attributes: &serde_json::Map<String, Value>,
        label: &str,
    ) -> Result<Self> {
        let mut values = BTreeMap::new();
        for key in REQUIRED_BUSINESS_BAGGAGE_KEYS
            .into_iter()
            .chain(OPTIONAL_BUSINESS_BAGGAGE_KEYS)
        {
            let Some(value) = attributes.get(key) else {
                if REQUIRED_BUSINESS_BAGGAGE_KEYS.contains(&key) {
                    bail!("{label} is missing `{key}`");
                }
                continue;
            };
            let value = value
                .as_str()
                .with_context(|| format!("{label} `{key}` is not a string"))?;
            ensure!(
                !value.trim().is_empty(),
                "{label} has an empty `{key}` value"
            );
            values.insert(key.to_owned(), value.to_owned());
        }
        Ok(Self { values })
    }

    fn ensure_matches(&self, actual: &BTreeMap<String, String>, label: &str) -> Result<()> {
        for (key, expected) in &self.values {
            ensure!(
                actual.get(key) == Some(expected),
                "{label} does not preserve business baggage `{key}`"
            );
        }
        if !self.values.contains_key("session.id") {
            ensure!(
                !actual.contains_key("session.id"),
                "{label} introduced an unexpected `session.id`"
            );
        }
        Ok(())
    }

    fn tenant_id(&self) -> &str {
        self.values
            .get("tenant.id")
            .map(String::as_str)
            .unwrap_or_default()
    }
}

pub(crate) async fn verify(api_url: &str, trace_id: &str) -> Result<Summary> {
    ensure!(
        is_trace_id(trace_id),
        "invalid OpenTelemetry trace id `{}`",
        bounded_output(trace_id)
    );
    let parallax_endpoint = format!("{}/graphql", api_url.trim_end_matches('/'));
    let storefront_endpoint = std::env::var("STOREFRONT_GRAPHQL_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_STOREFRONT_GRAPHQL_URL.to_owned());
    let timeout = configured_seconds(
        "COMMERCE_VERIFY_TIMEOUT_SECONDS",
        DEFAULT_VERIFY_TIMEOUT_SECONDS,
        MAX_VERIFY_TIMEOUT_SECONDS,
    )?;
    let poll_interval = configured_millis(
        "COMMERCE_VERIFY_POLL_INTERVAL_MILLIS",
        DEFAULT_POLL_INTERVAL_MILLIS,
        MAX_POLL_INTERVAL_MILLIS,
    )?;
    let tenant_id = std::env::var("COMMERCE_VERIFY_TENANT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_TENANT_ID.to_owned());
    let parallax_client = reqwest::Client::new();
    let storefront_client = reqwest::Client::new();
    let parallax_token = std::env::var("PARALLAX_API_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let deadline = Instant::now() + timeout;
    let last_error = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break String::from("verification deadline expired before another attempt");
        }
        match tokio::time::timeout(
            remaining,
            verify_attempt(
                &parallax_client,
                &storefront_client,
                &parallax_endpoint,
                &storefront_endpoint,
                &tenant_id,
                parallax_token.as_deref(),
                trace_id,
            ),
        )
        .await
        {
            Ok(Ok(summary)) => return Ok(summary),
            Ok(Err(error)) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break bounded_output(&error.to_string());
                }
                tokio::time::sleep(poll_interval.min(remaining)).await;
            }
            Err(_) => break String::from("verification attempt exceeded its deadline"),
        }
    };

    bail!(
        "commerce verification did not converge within {} seconds: {last_error}",
        timeout.as_secs()
    )
}

async fn verify_attempt(
    parallax_client: &reqwest::Client,
    storefront_client: &reqwest::Client,
    parallax_endpoint: &str,
    storefront_endpoint: &str,
    tenant_id: &str,
    parallax_token: Option<&str>,
    trace_id: &str,
) -> Result<Summary> {
    let traces =
        fetch_trace_graph(parallax_client, parallax_endpoint, parallax_token, trace_id).await?;
    let analysis = analyze_details(trace_id, &traces)?;
    verify_clickhouse_evidence(
        storefront_client,
        storefront_endpoint,
        tenant_id,
        trace_id,
        &analysis.business_baggage,
        &analysis.identity,
    )
    .await?;
    Ok(analysis.summary)
}

async fn fetch_trace_graph(
    client: &reqwest::Client,
    endpoint: &str,
    parallax_token: Option<&str>,
    trace_id: &str,
) -> Result<Vec<Value>> {
    let mut pending = VecDeque::from([trace_id.to_owned()]);
    let mut scheduled = HashSet::from([trace_id.to_owned()]);
    let mut fetched = HashSet::new();
    let mut traces = Vec::new();
    let mut total_spans: usize = 0;
    let mut total_typed_links: usize = 0;

    while let Some(current) = pending.pop_front() {
        if !fetched.insert(current.clone()) {
            continue;
        }
        ensure!(
            is_trace_id(&current),
            "Parallax returned an invalid linked trace id"
        );
        ensure!(
            fetched.len() <= MAX_TRACES,
            "commerce trace graph exceeds {MAX_TRACES} traces"
        );
        let response = parallax_graphql(
            client,
            endpoint,
            &format!(
                r#"{{ trace(traceId: {current:?}) {{ traceId spans {{ spanId parentSpanId name service kind statusCode attributes resource typedLinks {{ traceId spanId attributes }} }} }} linkedTraces(traceId: {current:?}) {{ traceId }} }}"#
            ),
            json!({}),
            parallax_token,
        )
        .await
        .with_context(|| format!("fetching Parallax trace {current}"))?;
        let trace = response
            .pointer("/data/trace")
            .filter(|value| !value.is_null())
            .cloned()
            .with_context(|| format!("Parallax has no trace {current}"))?;
        ensure!(
            trace.get("traceId").and_then(Value::as_str) == Some(current.as_str()),
            "Parallax returned trace data for a different trace than {current}"
        );
        let spans = trace
            .get("spans")
            .and_then(Value::as_array)
            .with_context(|| format!("Parallax trace {current} has no span list"))?;
        ensure!(
            spans.len() <= MAX_SPANS_PER_TRACE,
            "Parallax trace exceeds {MAX_SPANS_PER_TRACE} spans"
        );
        total_spans = total_spans
            .checked_add(spans.len())
            .context("Parallax trace span count overflowed")?;
        ensure!(
            total_spans <= MAX_TOTAL_SPANS,
            "commerce trace graph exceeds {MAX_TOTAL_SPANS} spans"
        );
        for span in spans {
            let links = span
                .get("typedLinks")
                .and_then(Value::as_array)
                .with_context(|| format!("Parallax trace {current} has invalid typed links"))?;
            ensure!(
                links.len() <= MAX_TYPED_LINKS_PER_SPAN,
                "Parallax trace has too many typed links on one span"
            );
            total_typed_links = total_typed_links
                .checked_add(links.len())
                .context("Parallax typed link count overflowed")?;
            ensure!(
                total_typed_links <= MAX_TOTAL_TYPED_LINKS,
                "commerce trace graph exceeds {MAX_TOTAL_TYPED_LINKS} typed links"
            );
        }
        let linked_traces = response
            .pointer("/data/linkedTraces")
            .and_then(Value::as_array)
            .context("Parallax response has no linked trace list")?;
        ensure!(
            linked_traces.len() <= MAX_TRACES,
            "Parallax returned too many linked traces"
        );
        let mut ids = Vec::new();
        for span in spans {
            ids.extend(linked_ids(span)?);
        }
        for value in linked_traces {
            let id = value
                .get("traceId")
                .and_then(Value::as_str)
                .context("Parallax returned a linked trace without a trace id")?;
            ensure!(
                is_trace_id(id),
                "Parallax returned an invalid linked trace id"
            );
            ids.push(id.to_owned());
        }
        for id in ids {
            ensure!(
                is_trace_id(&id),
                "Parallax returned an invalid linked trace id"
            );
            if scheduled.insert(id.clone()) {
                ensure!(
                    scheduled.len() <= MAX_TRACES,
                    "commerce trace graph exceeds {MAX_TRACES} traces"
                );
                pending.push_back(id);
            }
        }
        traces.push(trace);
    }

    Ok(traces)
}

fn configured_seconds(name: &str, default_seconds: u64, maximum_seconds: u64) -> Result<Duration> {
    let value = configured_positive(name)?.unwrap_or(default_seconds);
    ensure!(
        value <= maximum_seconds,
        "{name} must be no more than {maximum_seconds} seconds"
    );
    Ok(Duration::from_secs(value))
}

fn configured_millis(name: &str, default_millis: u64, maximum_millis: u64) -> Result<Duration> {
    let value = configured_positive(name)?.unwrap_or(default_millis);
    ensure!(
        value <= maximum_millis,
        "{name} must be no more than {maximum_millis} milliseconds"
    );
    Ok(Duration::from_millis(value))
}

fn configured_positive(name: &str) -> Result<Option<u64>> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(None);
    };
    let raw = raw
        .to_str()
        .with_context(|| format!("{name} must be valid UTF-8"))?;
    let value = raw
        .parse::<u64>()
        .with_context(|| format!("{name} must be a positive integer"))?;
    ensure!(value > 0, "{name} must be greater than zero");
    Ok(Some(value))
}

async fn parallax_graphql(
    client: &reqwest::Client,
    endpoint: &str,
    query: &str,
    variables: Value,
    bearer_token: Option<&str>,
) -> Result<Value> {
    graphql(client, endpoint, query, variables, bearer_token).await
}

async fn storefront_graphql(
    client: &reqwest::Client,
    endpoint: &str,
    query: &str,
    variables: Value,
) -> Result<Value> {
    // Storefront is a local application endpoint. It must never receive the
    // credential used for the optional external Parallax GraphQL API.
    graphql(client, endpoint, query, variables, None).await
}

async fn graphql(
    client: &reqwest::Client,
    endpoint: &str,
    query: &str,
    variables: Value,
    bearer_token: Option<&str>,
) -> Result<Value> {
    let mut request = client
        .post(endpoint)
        .json(&json!({"query": query, "variables": variables}));
    if let Some(token) = bearer_token.filter(|token| !token.trim().is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .await
        .with_context(|| format!("cannot reach GraphQL at {}", bounded_output(endpoint)))?
        .error_for_status()?
        .json::<Value>()
        .await
        .with_context(|| {
            format!(
                "GraphQL response from {} is invalid JSON",
                bounded_output(endpoint)
            )
        })?;
    if let Some(errors) = response
        .get("errors")
        .filter(|errors| !errors.as_array().is_none_or(Vec::is_empty))
    {
        bail!(
            "GraphQL error from {}: {}",
            bounded_output(endpoint),
            bounded_output(&errors.to_string())
        );
    }
    Ok(response)
}

fn bounded_output(value: &str) -> String {
    let mut output = value
        .chars()
        .take(MAX_ERROR_OUTPUT_CHARS)
        .collect::<String>();
    if value.chars().count() > MAX_ERROR_OUTPUT_CHARS {
        output.push('…');
    }
    output
}

async fn verify_clickhouse_evidence(
    client: &reqwest::Client,
    endpoint: &str,
    tenant_id: &str,
    trace_id: &str,
    expected_baggage: &BusinessBaggage,
    identity: &CommerceIdentity,
) -> Result<()> {
    let response = storefront_graphql(
        client,
        endpoint,
        ANALYTICS_QUERY,
        json!({"tenantId": tenant_id, "eventName": "order.paid"}),
    )
    .await
    .context("querying Storefront ClickHouse analytics")?;
    let events = response
        .pointer("/data/analyticsEvents")
        .and_then(Value::as_array)
        .context("Storefront analytics response has no event list")?;
    let event = events
        .iter()
        .find(|event| {
            event.get("traceId").and_then(Value::as_str) == Some(trace_id)
                && event.get("eventKey").and_then(Value::as_str)
                    == Some(identity.event_key.as_str())
        })
        .with_context(|| {
            format!(
                "ClickHouse has no order.paid event for trace {trace_id} and event key {}",
                bounded_output(&identity.event_key)
            )
        })?;
    validate_clickhouse_event_with_context(event, tenant_id, trace_id, expected_baggage, identity)
}

#[cfg(test)]
fn validate_clickhouse_event(event: &Value, tenant_id: &str, trace_id: &str) -> Result<()> {
    let baggage = validate_baggage(required_string(event, "baggage")?)?;
    let expected_baggage = BusinessBaggage::from_entries(&baggage, "ClickHouse baggage")?;
    let event_key = required_string(event, "eventKey")?.to_owned();
    validate_clickhouse_event_with_context(
        event,
        tenant_id,
        trace_id,
        &expected_baggage,
        &CommerceIdentity {
            tenant_id: tenant_id.to_owned(),
            event_id: required_string(event, "eventId")?.to_owned(),
            event_type: required_string(event, "eventName")?.to_owned(),
            event_key,
            order_id: required_string(event, "entityId")?.to_owned(),
        },
    )
}

fn validate_clickhouse_event_with_context(
    event: &Value,
    tenant_id: &str,
    trace_id: &str,
    expected_baggage: &BusinessBaggage,
    identity: &CommerceIdentity,
) -> Result<()> {
    let event_id = required_string(event, "eventId")?;
    let event_uuid = uuid::Uuid::parse_str(event_id)
        .context("ClickHouse event id is not a UUID analytics projection")?;
    ensure!(
        event_uuid.get_version_num() == 5,
        "ClickHouse event id is not a UUID v5 analytics projection"
    );
    ensure!(
        required_string(event, "tenantId")? == tenant_id,
        "ClickHouse event tenant does not match the verified tenant"
    );
    ensure!(
        expected_baggage.tenant_id() == tenant_id,
        "verified business baggage tenant does not match the verified tenant"
    );
    ensure!(
        identity.tenant_id == tenant_id,
        "verified commerce identity tenant does not match the verified tenant"
    );
    let event_key = required_string(event, "eventKey")?;
    ensure!(
        event_key == identity.event_key,
        "ClickHouse event key does not match the verified Rabbit event"
    );
    ensure!(
        required_string(event, "eventName")? == "order.paid",
        "ClickHouse evidence is not an order.paid event"
    );
    ensure!(
        required_string(event, "eventName")? == identity.event_type,
        "ClickHouse event type does not match the verified Rabbit event"
    );
    ensure!(
        identity.event_key == format!("{}:paid", identity.order_id),
        "verified Rabbit event key is not the canonical paid-order key"
    );
    ensure!(
        required_string(event, "source")? == "fulfillment",
        "ClickHouse evidence did not come from fulfillment"
    );
    ensure!(
        required_string(event, "entityType")? == "order",
        "ClickHouse evidence does not identify an order entity"
    );
    ensure!(
        !required_string(event, "entityId")?.is_empty(),
        "ClickHouse event entity is empty"
    );
    ensure!(
        required_string(event, "entityId")? == identity.order_id,
        "ClickHouse event order does not match the verified fulfillment order"
    );
    ensure!(
        required_string(event, "traceId")? == trace_id,
        "ClickHouse event trace id does not match the verified trace"
    );

    let traceparent = required_string(event, "traceparent")?;
    validate_traceparent(traceparent, trace_id)?;
    let span_id = required_string(event, "spanId")?;
    ensure!(
        is_hex(span_id, 16) && !all_zero(span_id),
        "ClickHouse event span id is invalid"
    );
    ensure!(
        traceparent.split('-').nth(2) == Some(span_id),
        "ClickHouse event span id does not match W3C traceparent"
    );
    let tracestate = required_string(event, "tracestate")?;
    validate_tracestate(tracestate)?;
    let baggage = required_string(event, "baggage")?;
    let baggage_items = validate_baggage(baggage)?;
    expected_baggage.ensure_matches(&baggage_items, "ClickHouse baggage")?;

    let properties = required_string(event, "properties")?;
    ensure!(properties != "{}", "ClickHouse event properties are empty");
    let context_raw = required_string(event, "context")?;
    ensure!(
        context_raw != "{}",
        "ClickHouse propagation context is empty"
    );
    let context = serde_json::from_str::<Value>(context_raw)
        .context("ClickHouse propagation context is invalid JSON")?;
    for (key, expected) in [
        ("traceparent", traceparent),
        ("tracestate", tracestate),
        ("baggage", baggage),
    ] {
        ensure!(
            context.get(key).and_then(Value::as_str) == Some(expected),
            "ClickHouse propagation context does not preserve {key}"
        );
    }
    let context_baggage = context
        .get("baggage_items")
        .and_then(Value::as_object)
        .context("ClickHouse propagation context has no baggage_items")?;
    let context_baggage = string_map(context_baggage, "ClickHouse baggage_items")?;
    expected_baggage.ensure_matches(&context_baggage, "ClickHouse baggage_items")?;
    Ok(())
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("ClickHouse analytics field `{field}` is missing or empty"))
}

fn span_attribute_object(span: &Value) -> Result<serde_json::Map<String, Value>> {
    let raw = span
        .get("attributes")
        .and_then(Value::as_str)
        .context("Parallax span is missing attributes")?;
    ensure!(
        raw.len() <= MAX_ATTRIBUTE_JSON_BYTES,
        "Parallax span attributes exceed the bounded size"
    );
    let attributes =
        serde_json::from_str::<Value>(raw).context("Parallax span attributes are invalid JSON")?;
    attributes
        .as_object()
        .cloned()
        .context("Parallax span attributes are not an object")
}

fn required_attribute_string(span: &Value, key: &str, label: &str) -> Result<String> {
    span_attribute_object(span)?
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .with_context(|| format!("{label} is missing `{key}`"))
}

fn string_map(
    object: &serde_json::Map<String, Value>,
    label: &str,
) -> Result<BTreeMap<String, String>> {
    let mut entries = BTreeMap::new();
    for (key, value) in object {
        let value = value
            .as_str()
            .with_context(|| format!("{label} `{key}` is not a string"))?;
        ensure!(valid_w3c_key(key), "{label} has an invalid key `{key}`");
        ensure!(!value.trim().is_empty(), "{label} `{key}` is empty");
        ensure!(
            value.chars().all(|character| {
                !character.is_control() && character != ',' && character != ';'
            }),
            "{label} `{key}` has an invalid value"
        );
        ensure!(entries.insert(key.clone(), value.to_owned()).is_none());
    }
    Ok(entries)
}

fn validate_traceparent(value: &str, expected_trace_id: &str) -> Result<()> {
    let parts = value.split('-').collect::<Vec<_>>();
    ensure!(
        parts.len() == 4 && parts[0] == "00",
        "invalid W3C traceparent version or shape"
    );
    ensure!(
        parts[1].eq_ignore_ascii_case(expected_trace_id)
            && is_hex(parts[1], 32)
            && !all_zero(parts[1]),
        "W3C traceparent trace id does not match the verified trace"
    );
    ensure!(
        is_hex(parts[2], 16) && !all_zero(parts[2]),
        "invalid W3C traceparent span id"
    );
    ensure!(is_hex(parts[3], 2), "invalid W3C traceparent flags");
    Ok(())
}

fn validate_tracestate(value: &str) -> Result<BTreeMap<String, String>> {
    ensure!(
        value.len() <= MAX_TRACESTATE_HEADER_BYTES,
        "W3C tracestate exceeds {MAX_TRACESTATE_HEADER_BYTES} bytes"
    );
    let members = value.split(',').collect::<Vec<_>>();
    ensure!(
        !members.is_empty() && members.len() <= MAX_TRACESTATE_MEMBERS,
        "W3C tracestate has invalid members"
    );
    let mut entries = BTreeMap::new();
    for raw_member in members {
        let member = trim_ows(raw_member);
        let Some((key, member_value)) = member.split_once('=') else {
            bail!("invalid W3C tracestate member");
        };
        ensure!(
            !member.is_empty()
                && !key.is_empty()
                && member.len() <= MAX_TRACESTATE_MEMBER_BYTES
                && !member_value.is_empty()
                && !member_value.contains('=')
                && valid_tracestate_key(key)
                && member_value.len() <= MAX_TRACESTATE_VALUE_BYTES
                && valid_tracestate_value(member_value)
                && entries
                    .insert(key.to_owned(), member_value.to_owned())
                    .is_none(),
            "invalid W3C tracestate member"
        );
    }
    Ok(entries)
}

fn validate_baggage(value: &str) -> Result<BTreeMap<String, String>> {
    ensure!(
        value.len() <= 8_192,
        "W3C baggage exceeds the bounded header size"
    );
    let mut entries = BTreeMap::new();
    for member in value.split(',') {
        let member = member.trim();
        let Some((key, raw_value)) = member.split_once('=') else {
            bail!("invalid W3C baggage member");
        };
        let value = raw_value.split(';').next().unwrap_or_default().trim();
        ensure!(
            valid_w3c_key(key) && !value.is_empty(),
            "invalid W3C baggage member"
        );
        ensure!(
            value.chars().all(|character| {
                !character.is_control() && character != ',' && character != ';'
            }),
            "invalid W3C baggage value"
        );
        ensure!(
            entries.insert(key.to_owned(), value.to_owned()).is_none(),
            "duplicate W3C baggage key"
        );
    }
    ensure!(!entries.is_empty(), "W3C baggage is empty");
    Ok(entries)
}

fn valid_tracestate_key(value: &str) -> bool {
    let mut parts = value.split('@');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    parts.next().is_none()
        && valid_tracestate_key_part(first, if second.is_some() { 241 } else { 256 })
        && second.is_none_or(|part| valid_tracestate_key_part(part, 14))
}

fn valid_tracestate_key_part(value: &str, max_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_length
        && value
            .as_bytes()
            .first()
            .copied()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.as_bytes().iter().skip(1).copied().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_*/-".contains(&byte)
        })
}

fn valid_tracestate_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.last().copied() != Some(b' ')
        && bytes.iter().copied().all(|byte| {
            (0x20..=0x2b).contains(&byte)
                || (0x2d..=0x3c).contains(&byte)
                || (0x3e..=0x7e).contains(&byte)
        })
}

fn valid_w3c_key(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'/' | b'@')
        })
}

fn trim_ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn all_zero(value: &str) -> bool {
    value.bytes().all(|byte| byte == b'0')
}

fn is_span_id(value: &str) -> bool {
    is_hex(value, 16) && !all_zero(value)
}

#[derive(Debug, Clone, Copy)]
struct SpanNode<'a> {
    trace_id: &'a str,
    span_id: &'a str,
    parent_span_id: Option<&'a str>,
    span: &'a Value,
}

type SpanKey<'a> = (&'a str, &'a str);

#[derive(Debug, Clone)]
struct SpanLink {
    source: usize,
    target: usize,
    attributes: Value,
}

#[cfg(test)]
fn analyze(anchor: &str, traces: &[Value]) -> Result<Summary> {
    Ok(analyze_details(anchor, traces)?.summary)
}

fn analyze_details(anchor: &str, traces: &[Value]) -> Result<Analysis> {
    ensure!(is_trace_id(anchor), "commerce trace id is invalid");
    let nodes = collect_span_nodes(anchor, traces)?;
    let index = index_spans(&nodes)?;
    let roots = validate_parent_topology(&nodes, &index)?;
    let links = validate_typed_links(&nodes, &index)?;
    validate_causal_connectivity(anchor, &nodes, &index, &roots, &links)?;

    let spans = nodes.iter().map(|node| node.span).collect::<Vec<_>>();

    let services = spans
        .iter()
        .filter_map(|span| span.get("service").and_then(Value::as_str))
        .filter(|service| !service.is_empty())
        .collect::<BTreeSet<_>>();
    for required in ["checkout", "catalog", "pricing", "inventory", "payment"] {
        ensure!(
            services.contains(required),
            "commerce trace is missing service `{required}`"
        );
    }
    // A direct checkout trace intentionally has no browser hop. If either
    // canonical browser service is present, accepting the other as absent
    // would make a partial browser topology look complete.
    if services.contains("web") || services.contains("storefront") {
        for required in ["web", "storefront"] {
            ensure!(
                services.contains(required),
                "canonical browser trace is missing service `{required}`"
            );
        }
    }

    let names = spans
        .iter()
        .filter_map(|span| span.get("name").and_then(Value::as_str))
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    for required in ["checkout", "pricing.quote", "inventory.reserve"] {
        ensure!(
            names.iter().any(|name| name == required),
            "commerce trace is missing span `{required}`"
        );
    }
    ensure!(
        spans.iter().any(|span| {
            span.get("service").and_then(Value::as_str) == Some("payment")
                && searchable_span_text(span).contains("authorize")
        }),
        "commerce trace is missing payment authorization evidence"
    );
    let checkout_producers = matching_span_indices(
        &nodes,
        "checkout",
        &[
            ("messaging.system", "rabbitmq"),
            ("messaging.destination.name", "commerce.events"),
            ("messaging.operation.name", "send"),
            ("commerce.event.type", "order.paid"),
        ],
    );
    ensure!(
        !checkout_producers.is_empty(),
        "commerce trace is missing the checkout RabbitMQ producer"
    );
    let order_consumers = matching_span_indices(
        &nodes,
        "fulfillment",
        &[
            ("messaging.system", "rabbitmq"),
            ("messaging.destination.name", "fulfillment.orders"),
            ("messaging.operation.name", "process"),
            ("commerce.event.type", "order.paid"),
        ],
    );
    ensure!(
        !order_consumers.is_empty(),
        "commerce trace is missing fulfillment RabbitMQ consumer evidence"
    );
    let analytics_consumers = matching_span_indices(
        &nodes,
        "fulfillment",
        &[
            ("messaging.system", "rabbitmq"),
            ("messaging.destination.name", "analytics.events"),
            ("messaging.operation.name", "process"),
            ("commerce.event.type", "order.paid"),
        ],
    );
    ensure!(
        !analytics_consumers.is_empty(),
        "commerce trace is missing the fulfillment analytics consumer"
    );
    require_link_semantics(
        &links,
        &order_consumers,
        &checkout_producers,
        "fulfillment order consumers are not linked to the checkout producer",
    )?;
    require_link_semantics(
        &links,
        &analytics_consumers,
        &checkout_producers,
        "fulfillment analytics consumers are not linked to the checkout producer",
    )?;

    let checkout_root = checkout_span_index(anchor, &nodes)?;
    let business_baggage = BusinessBaggage::from_attribute_object(
        &span_attribute_object(nodes[checkout_root].span)?,
        "checkout business baggage",
    )?;
    for position in checkout_producers
        .iter()
        .chain(order_consumers.iter())
        .chain(analytics_consumers.iter())
    {
        require_span_business_baggage(
            nodes[*position].span,
            &business_baggage,
            &format!("{} span", nodes[*position].span["service"]),
        )?;
    }
    let order_tracestate = validate_checkout_rabbit_edges(
        &links,
        &nodes,
        &checkout_producers,
        &order_consumers,
        &business_baggage,
    )?;
    let analytics_tracestate = validate_checkout_rabbit_edges(
        &links,
        &nodes,
        &checkout_producers,
        &analytics_consumers,
        &business_baggage,
    )?;
    ensure!(
        order_tracestate == analytics_tracestate,
        "checkout RabbitMQ edges do not share one W3C tracestate"
    );

    let identity = verified_commerce_identity(
        &nodes,
        &checkout_producers,
        &order_consumers,
        &analytics_consumers,
        &business_baggage,
    )?;
    require_notification_evidence(
        &nodes,
        &index,
        &order_consumers,
        &business_baggage,
        &identity,
    )?;

    let mut service_names = services.into_iter().map(str::to_owned).collect::<Vec<_>>();
    service_names.sort();
    Ok(Analysis {
        summary: Summary {
            traces: traces.len(),
            spans: spans.len(),
            services: service_names,
        },
        business_baggage,
        identity,
    })
}

fn checkout_span_index(anchor: &str, nodes: &[SpanNode<'_>]) -> Result<usize> {
    let checkout_spans = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.trace_id == anchor
                && node.span.get("service").and_then(Value::as_str) == Some("checkout")
                && node.span.get("name").and_then(Value::as_str) == Some("checkout")
        })
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    ensure!(
        checkout_spans.len() == 1,
        "commerce checkout trace does not have exactly one checkout span"
    );
    Ok(checkout_spans[0])
}

fn require_span_business_baggage(
    span: &Value,
    expected: &BusinessBaggage,
    label: &str,
) -> Result<()> {
    let actual = BusinessBaggage::from_attribute_object(
        &span_attribute_object(span)?,
        &format!("{label} business baggage"),
    )?;
    expected.ensure_matches(&actual.values, &format!("{label} business baggage"))
}

fn verified_commerce_identity(
    nodes: &[SpanNode<'_>],
    checkout_producers: &[usize],
    order_consumers: &[usize],
    analytics_consumers: &[usize],
    expected_baggage: &BusinessBaggage,
) -> Result<CommerceIdentity> {
    let event_id = required_attribute_string(
        nodes[checkout_producers[0]].span,
        "messaging.message.id",
        "checkout RabbitMQ producer",
    )?;
    for position in checkout_producers {
        ensure!(
            required_attribute_string(
                nodes[*position].span,
                "messaging.message.id",
                "checkout RabbitMQ producer",
            )? == event_id,
            "checkout RabbitMQ producers do not share one event identity"
        );
    }

    let order_consumer = nodes[order_consumers[0]].span;
    let identity = CommerceIdentity {
        tenant_id: expected_baggage.tenant_id().to_owned(),
        event_id,
        event_type: required_attribute_string(
            order_consumer,
            "commerce.event.type",
            "fulfillment order consumer",
        )?,
        order_id: required_attribute_string(
            order_consumer,
            "order.id",
            "fulfillment order consumer",
        )?,
        event_key: String::new(),
    };
    ensure!(
        identity.event_type == "order.paid",
        "verified fulfillment event is not order.paid"
    );
    let mut identity = identity;
    identity.event_key = format!("{}:paid", identity.order_id);

    for position in order_consumers {
        ensure_span_identity(nodes[*position].span, &identity, true)?;
    }
    for position in analytics_consumers {
        ensure_span_identity(nodes[*position].span, &identity, false)?;
        let attributes = span_attribute_object(nodes[*position].span)?;
        if let Some(order_id) = attributes.get("order.id") {
            ensure!(
                order_id.as_str() == Some(identity.order_id.as_str()),
                "fulfillment analytics consumer order does not match the verified order"
            );
        }
    }
    Ok(identity)
}

fn ensure_span_identity(
    span: &Value,
    identity: &CommerceIdentity,
    require_order: bool,
) -> Result<()> {
    ensure!(
        required_attribute_string(span, "tenant.id", "commerce span")? == identity.tenant_id,
        "commerce span tenant does not match the verified tenant"
    );
    ensure!(
        required_attribute_string(span, "messaging.message.id", "commerce span")?
            == identity.event_id,
        "commerce span message does not match the verified event"
    );
    ensure!(
        required_attribute_string(span, "commerce.event.type", "commerce span")?
            == identity.event_type,
        "commerce span event type does not match the verified event"
    );
    if require_order {
        ensure!(
            required_attribute_string(span, "order.id", "commerce span")? == identity.order_id,
            "commerce span order does not match the verified order"
        );
    }
    Ok(())
}

fn validate_checkout_rabbit_edges(
    links: &[SpanLink],
    nodes: &[SpanNode<'_>],
    checkout_producers: &[usize],
    consumers: &[usize],
    expected_baggage: &BusinessBaggage,
) -> Result<BTreeMap<String, String>> {
    let mut expected_tracestate = None;
    let mut edge_count = 0;
    for link in links.iter().filter(|link| {
        consumers.contains(&link.source) && checkout_producers.contains(&link.target)
    }) {
        edge_count += 1;
        validate_rabbit_edge(link, nodes, expected_baggage, &mut expected_tracestate)?;
    }
    ensure!(
        edge_count == consumers.len(),
        "not every required checkout RabbitMQ edge has a carrier"
    );
    expected_tracestate.context("required checkout RabbitMQ edge has no tracestate")
}

fn validate_rabbit_edge(
    link: &SpanLink,
    nodes: &[SpanNode<'_>],
    expected_baggage: &BusinessBaggage,
    expected_tracestate: &mut Option<BTreeMap<String, String>>,
) -> Result<()> {
    let target = &nodes[link.target];
    let source = &nodes[link.source];
    let carrier = carrier_from_attributes(&link.attributes)?;
    validate_traceparent_for_span(&carrier.traceparent, target.trace_id, target.span_id)?;
    expected_baggage.ensure_matches(
        &carrier.baggage,
        &format!("RabbitMQ carrier {} -> {}", source.span_id, target.span_id),
    )?;
    if let Some(expected) = expected_tracestate {
        ensure!(
            &carrier.tracestate == expected,
            "RabbitMQ tracestate changed between checkout edges"
        );
    } else {
        *expected_tracestate = Some(carrier.tracestate);
    }
    Ok(())
}

fn carrier_from_attributes(attributes: &Value) -> Result<W3cCarrier> {
    let object = attributes
        .as_object()
        .context("RabbitMQ edge carrier is not an object")?;
    let object = object
        .get("carrier")
        .filter(|value| value.is_object())
        .and_then(Value::as_object)
        .unwrap_or(object);
    let traceparent = required_carrier_string(object, "traceparent")?;
    let tracestate_value = required_carrier_string(object, "tracestate")?;
    let baggage_value = required_carrier_string(object, "baggage")?;
    let tracestate = validate_tracestate(&tracestate_value)?;
    let baggage = validate_baggage(&baggage_value)?;
    Ok(W3cCarrier {
        traceparent,
        tracestate,
        baggage,
    })
}

fn required_carrier_string(object: &serde_json::Map<String, Value>, field: &str) -> Result<String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .with_context(|| format!("RabbitMQ edge carrier is missing `{field}`"))
}

fn validate_traceparent_for_span(
    value: &str,
    expected_trace_id: &str,
    expected_span_id: &str,
) -> Result<()> {
    validate_traceparent(value, expected_trace_id)?;
    ensure!(
        value.split('-').nth(2) == Some(expected_span_id),
        "RabbitMQ traceparent does not identify its checkout producer"
    );
    Ok(())
}

fn require_notification_evidence(
    nodes: &[SpanNode<'_>],
    index: &HashMap<SpanKey<'_>, usize>,
    order_consumers: &[usize],
    expected_baggage: &BusinessBaggage,
    identity: &CommerceIdentity,
) -> Result<()> {
    let notification_spans = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.span.get("service").and_then(Value::as_str) == Some("notifications")
        })
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    ensure!(
        !notification_spans.is_empty(),
        "commerce trace is missing notification delivery evidence"
    );

    for position in notification_spans {
        if !has_parent_ancestor(position, nodes, index, order_consumers)? {
            continue;
        }
        if require_span_business_baggage(nodes[position].span, expected_baggage, "notification")
            .is_err()
        {
            continue;
        }
        let Ok(attributes) = span_attribute_object(nodes[position].span) else {
            continue;
        };
        if [
            ("order.id", identity.order_id.as_str()),
            ("commerce.event.type", identity.event_type.as_str()),
            ("messaging.message.id", identity.event_id.as_str()),
        ]
        .iter()
        .any(|(key, expected)| {
            attributes
                .get(*key)
                .is_some_and(|actual| actual.as_str() != Some(*expected))
        }) {
            continue;
        }
        return Ok(());
    }

    bail!("notification evidence is not causally connected to the verified order/event/tenant")
}

fn has_parent_ancestor(
    start: usize,
    nodes: &[SpanNode<'_>],
    index: &HashMap<SpanKey<'_>, usize>,
    ancestors: &[usize],
) -> Result<bool> {
    let mut current = start;
    let mut seen = HashSet::new();
    loop {
        let Some(parent) = nodes[current].parent_span_id else {
            return Ok(false);
        };
        current = *index
            .get(&(nodes[current].trace_id, parent))
            .context("Parallax parentSpanId lookup failed")?;
        if ancestors.contains(&current) {
            return Ok(true);
        }
        ensure!(
            seen.insert(current),
            "Parallax notification parent chain contains a cycle"
        );
    }
}

fn collect_span_nodes<'a>(anchor: &str, traces: &'a [Value]) -> Result<Vec<SpanNode<'a>>> {
    ensure!(!traces.is_empty(), "commerce trace {anchor} has no traces");
    ensure!(
        traces.len() <= MAX_TRACES,
        "commerce trace graph exceeds {MAX_TRACES} traces"
    );

    let mut trace_ids = BTreeSet::new();
    let mut nodes = Vec::new();
    for trace in traces {
        let trace_id = trace
            .get("traceId")
            .and_then(Value::as_str)
            .context("Parallax trace is missing traceId")?;
        ensure!(
            is_trace_id(trace_id),
            "Parallax returned an invalid trace id"
        );
        ensure!(
            trace_ids.insert(trace_id),
            "Parallax returned a duplicate trace"
        );

        let spans = trace
            .get("spans")
            .and_then(Value::as_array)
            .context("Parallax trace has no span list")?;
        ensure!(!spans.is_empty(), "Parallax trace has no spans");
        ensure!(
            spans.len() <= MAX_SPANS_PER_TRACE,
            "Parallax trace exceeds {MAX_SPANS_PER_TRACE} spans"
        );
        ensure!(
            nodes.len().saturating_add(spans.len()) <= MAX_TOTAL_SPANS,
            "commerce trace graph exceeds {MAX_TOTAL_SPANS} spans"
        );

        for span in spans {
            let span_id = span
                .get("spanId")
                .and_then(Value::as_str)
                .context("Parallax span is missing spanId")?;
            ensure!(is_span_id(span_id), "Parallax returned an invalid span id");
            for field in ["name", "service"] {
                ensure!(
                    span.get(field)
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty()),
                    "Parallax span has a missing or empty {field}"
                );
            }
            validate_span_attributes(span)?;
            let parent_span_id = parse_parent_span_id(span)?;
            let typed_links = span
                .get("typedLinks")
                .and_then(Value::as_array)
                .context("Parallax span is missing typedLinks")?;
            ensure!(
                typed_links.len() <= MAX_TYPED_LINKS_PER_SPAN,
                "Parallax span has too many typed links"
            );
            nodes.push(SpanNode {
                trace_id,
                span_id,
                parent_span_id,
                span,
            });
        }
    }
    ensure!(
        trace_ids.iter().any(|trace_id| *trace_id == anchor),
        "commerce trace graph does not contain its anchor trace"
    );
    Ok(nodes)
}

fn parse_parent_span_id(span: &Value) -> Result<Option<&str>> {
    let value = span
        .get("parentSpanId")
        .context("Parallax span is missing parentSpanId")?;
    if value.is_null() {
        return Ok(None);
    }
    let parent = value
        .as_str()
        .context("Parallax span parentSpanId is not a string")?;
    ensure!(
        is_span_id(parent),
        "Parallax span has an invalid parentSpanId"
    );
    Ok(Some(parent))
}

fn validate_span_attributes(span: &Value) -> Result<()> {
    let raw = span
        .get("attributes")
        .and_then(Value::as_str)
        .context("Parallax span is missing attributes")?;
    ensure!(
        raw.len() <= MAX_ATTRIBUTE_JSON_BYTES,
        "Parallax span attributes exceed the bounded size"
    );
    let parsed =
        serde_json::from_str::<Value>(raw).context("Parallax span attributes are invalid JSON")?;
    ensure!(
        parsed.is_object(),
        "Parallax span attributes are not an object"
    );
    Ok(())
}

fn index_spans<'a>(nodes: &[SpanNode<'a>]) -> Result<HashMap<SpanKey<'a>, usize>> {
    let mut index = HashMap::with_capacity(nodes.len());
    for (position, node) in nodes.iter().enumerate() {
        ensure!(
            index
                .insert((node.trace_id, node.span_id), position)
                .is_none(),
            "Parallax returned duplicate span identity"
        );
    }
    Ok(index)
}

fn validate_parent_topology<'a>(
    nodes: &[SpanNode<'a>],
    index: &HashMap<SpanKey<'a>, usize>,
) -> Result<Vec<usize>> {
    let mut roots_by_trace = BTreeMap::<&str, Vec<usize>>::new();
    for (position, node) in nodes.iter().enumerate() {
        if let Some(parent) = node.parent_span_id {
            ensure!(
                index.contains_key(&(node.trace_id, parent)),
                "Parallax parentSpanId does not reference a span in its trace"
            );
        } else {
            roots_by_trace
                .entry(node.trace_id)
                .or_default()
                .push(position);
        }
    }

    let trace_count = nodes
        .iter()
        .map(|node| node.trace_id)
        .collect::<BTreeSet<_>>()
        .len();
    ensure!(
        roots_by_trace.len() == trace_count,
        "Parallax trace graph has a trace without a parent root"
    );
    for roots in roots_by_trace.values() {
        ensure!(
            roots.len() == 1,
            "Parallax trace has multiple disconnected parent roots"
        );
    }

    for start in 0..nodes.len() {
        let mut seen = HashSet::new();
        let mut current = start;
        loop {
            ensure!(
                seen.insert(current),
                "Parallax parent chain contains a cycle"
            );
            let Some(parent) = nodes[current].parent_span_id else {
                break;
            };
            current = *index
                .get(&(nodes[current].trace_id, parent))
                .context("Parallax parentSpanId lookup failed")?;
        }
    }

    Ok(roots_by_trace.values().flatten().copied().collect())
}

fn validate_typed_links<'a>(
    nodes: &[SpanNode<'a>],
    index: &HashMap<SpanKey<'a>, usize>,
) -> Result<Vec<SpanLink>> {
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    let mut total_links = 0usize;
    for (source, node) in nodes.iter().enumerate() {
        let raw_links = node
            .span
            .get("typedLinks")
            .and_then(Value::as_array)
            .context("Parallax span is missing typedLinks")?;
        ensure!(
            raw_links.len() <= MAX_TYPED_LINKS_PER_SPAN,
            "Parallax span has too many typed links"
        );
        total_links = total_links
            .checked_add(raw_links.len())
            .context("Parallax typed link count overflowed")?;
        ensure!(
            total_links <= MAX_TOTAL_TYPED_LINKS,
            "commerce trace graph exceeds {MAX_TOTAL_TYPED_LINKS} typed links"
        );
        for link in raw_links {
            let link = link
                .as_object()
                .context("Parallax typed link is not an object")?;
            let trace_id = link
                .get("traceId")
                .and_then(Value::as_str)
                .context("Parallax typed link is missing traceId")?;
            ensure!(
                is_trace_id(trace_id),
                "Parallax typed link has an invalid traceId"
            );
            let span_id = link
                .get("spanId")
                .and_then(Value::as_str)
                .context("Parallax typed link is missing spanId")?;
            ensure!(
                is_span_id(span_id),
                "Parallax typed link has an invalid spanId"
            );
            let attributes = validate_link_attributes(link)?;
            let target = *index
                .get(&(trace_id, span_id))
                .context("Parallax typed link target is not in the fetched graph")?;
            ensure!(
                source != target,
                "Parallax typed link cannot point to its source span"
            );
            validate_link_semantics(nodes[source].span, nodes[target].span)?;
            ensure!(
                seen.insert((source, target)),
                "Parallax returned a duplicate typed link"
            );
            links.push(SpanLink {
                source,
                target,
                attributes,
            });
        }
    }
    Ok(links)
}

fn validate_link_attributes(link: &serde_json::Map<String, Value>) -> Result<Value> {
    let attributes = link
        .get("attributes")
        .context("Parallax typed link is missing attributes")?;
    let raw = attributes
        .as_str()
        .context("Parallax typed link attributes are not a JSON string")?;
    ensure!(
        raw.len() <= MAX_LINK_ATTRIBUTES_BYTES,
        "Parallax typed link attributes exceed the bounded size"
    );
    let parsed = serde_json::from_str::<Value>(raw)
        .context("Parallax typed link attributes are invalid JSON")?;
    ensure!(
        parsed.is_object(),
        "Parallax typed link attributes are not an object"
    );
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessagingRole {
    Producer,
    Consumer,
}

fn messaging_role(span: &Value) -> Option<MessagingRole> {
    if is_rabbitmq_operation(span, "send")
        && span.get("kind").and_then(Value::as_str) == Some("SPAN_KIND_PRODUCER")
    {
        Some(MessagingRole::Producer)
    } else if is_rabbitmq_operation(span, "process")
        && span.get("kind").and_then(Value::as_str) == Some("SPAN_KIND_CONSUMER")
    {
        Some(MessagingRole::Consumer)
    } else {
        None
    }
}

fn is_rabbitmq_operation(span: &Value, operation: &str) -> bool {
    attribute_string(span, "messaging.system").as_deref() == Some("rabbitmq")
        && attribute_string(span, "messaging.operation.name").as_deref() == Some(operation)
}

fn validate_link_semantics(source: &Value, target: &Value) -> Result<()> {
    let source_role = messaging_role(source)
        .context("Parallax typed link source is not a RabbitMQ producer or consumer")?;
    let target_role = messaging_role(target)
        .context("Parallax typed link target is not a RabbitMQ producer or consumer")?;
    ensure!(
        source_role == MessagingRole::Consumer && target_role == MessagingRole::Producer,
        "Parallax typed link must point from a RabbitMQ consumer to a producer"
    );

    let source_message_id = attribute_string(source, "messaging.message.id")
        .context("Parallax typed link source has no message identity")?;
    let target_message_id = attribute_string(target, "messaging.message.id")
        .context("Parallax typed link target has no message identity")?;
    ensure!(
        !source_message_id.trim().is_empty()
            && !target_message_id.trim().is_empty()
            && source_message_id == target_message_id,
        "Parallax typed link message identities do not match"
    );
    Ok(())
}

fn validate_causal_connectivity<'a>(
    anchor: &str,
    nodes: &[SpanNode<'a>],
    index: &HashMap<SpanKey<'a>, usize>,
    roots: &[usize],
    links: &[SpanLink],
) -> Result<()> {
    let anchor_roots = roots
        .iter()
        .copied()
        .filter(|root| nodes[*root].trace_id == anchor)
        .collect::<Vec<_>>();
    ensure!(
        anchor_roots.len() == 1,
        "commerce anchor trace does not have exactly one root"
    );

    let mut adjacency = vec![Vec::new(); nodes.len()];
    for (child, node) in nodes.iter().enumerate() {
        if let Some(parent) = node.parent_span_id {
            let parent = *index
                .get(&(node.trace_id, parent))
                .context("Parallax parentSpanId lookup failed")?;
            adjacency[child].push(parent);
            adjacency[parent].push(child);
        }
    }
    for link in links {
        adjacency[link.source].push(link.target);
        adjacency[link.target].push(link.source);
    }

    let mut reachable = HashSet::new();
    let mut frontier = VecDeque::from(anchor_roots);
    while let Some(position) = frontier.pop_front() {
        if !reachable.insert(position) {
            continue;
        }
        frontier.extend(adjacency[position].iter().copied());
    }
    ensure!(
        reachable.len() == nodes.len(),
        "commerce trace has disconnected causal spans (reachable {} of {})",
        reachable.len(),
        nodes.len()
    );
    Ok(())
}

fn matching_span_indices(
    nodes: &[SpanNode<'_>],
    service: &str,
    expected: &[(&str, &str)],
) -> Vec<usize> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| {
            node.span.get("service").and_then(Value::as_str) == Some(service)
                && expected
                    .iter()
                    .all(|(key, value)| attribute_string(node.span, key).as_deref() == Some(*value))
        })
        .map(|(position, _)| position)
        .collect()
}

fn require_link_semantics(
    links: &[SpanLink],
    sources: &[usize],
    targets: &[usize],
    message: &str,
) -> Result<()> {
    ensure!(!sources.is_empty(), "commerce trace has no link sources");
    ensure!(!targets.is_empty(), "commerce trace has no link targets");
    ensure!(
        sources.iter().all(|source| {
            links
                .iter()
                .any(|link| link.source == *source && targets.contains(&link.target))
        }),
        "{message}"
    );
    Ok(())
}

fn linked_ids(span: &Value) -> Result<Vec<String>> {
    let links = span
        .get("typedLinks")
        .and_then(Value::as_array)
        .context("Parallax span has no typed link list")?;
    ensure!(
        links.len() <= MAX_TYPED_LINKS_PER_SPAN,
        "Parallax span has too many typed links"
    );
    links
        .iter()
        .map(|link| {
            let trace_id = link
                .get("traceId")
                .and_then(Value::as_str)
                .context("Parallax typed link has no trace id")?;
            ensure!(
                is_trace_id(trace_id),
                "Parallax typed link has an invalid trace id"
            );
            Ok(trace_id.to_owned())
        })
        .collect()
}

fn attribute_string(span: &Value, key: &str) -> Option<String> {
    let raw = span.get("attributes").and_then(Value::as_str)?;
    if raw.len() > MAX_ATTRIBUTE_JSON_BYTES {
        return None;
    }
    let attributes = serde_json::from_str::<Value>(raw).ok()?;
    attributes
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn searchable_span_text(span: &Value) -> String {
    let mut values = Vec::new();
    for key in ["name", "kind", "service"] {
        if let Some(value) = span.get(key).and_then(Value::as_str) {
            values.push(
                value
                    .chars()
                    .take(MAX_ATTRIBUTE_JSON_BYTES)
                    .collect::<String>()
                    .to_ascii_lowercase(),
            );
        }
    }
    if let Some(raw) = span.get("attributes").and_then(Value::as_str) {
        values.push(
            raw.chars()
                .take(MAX_ATTRIBUTE_JSON_BYTES)
                .collect::<String>()
                .to_ascii_lowercase(),
        );
    }
    values.join(" ")
}

fn is_trace_id(value: &str) -> bool {
    is_hex(value, 32) && !all_zero(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
    const FULFILLMENT_TRACE_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ANALYTICS_TRACE_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    const CHECKOUT_ROOT: &str = "0000000000000001";
    const CATALOG_SPAN: &str = "0000000000000002";
    const PRICING_SPAN: &str = "0000000000000003";
    const INVENTORY_SPAN: &str = "0000000000000004";
    const PAYMENT_SPAN: &str = "0000000000000005";
    const CHECKOUT_PRODUCER: &str = "0000000000000006";
    const ORDER_CONSUMER: &str = "0000000000000010";
    const NOTIFICATION_SPAN: &str = "0000000000000011";
    const FULFILLMENT_PRODUCER: &str = "0000000000000012";
    const ANALYTICS_CONSUMER: &str = "0000000000000020";
    const WEB_SPAN: &str = "0000000000000030";
    const STOREFRONT_SPAN: &str = "0000000000000031";
    const BUSINESS_BAGGAGE: &str = "tenant.id=tenant-acme,user.tier=standard,customer.segment=standard,region=us-east-1,request.priority=normal,session.id=session-1";

    fn link(trace_id: &str, span_id: &str) -> Value {
        json!({
            "traceId": trace_id,
            "spanId": span_id,
            "attributes": json!({
                "traceparent": format!("00-{trace_id}-{span_id}-01"),
                "tracestate": "playground=commerce",
                "baggage": BUSINESS_BAGGAGE
            }).to_string()
        })
    }

    fn span(
        span_id: &str,
        parent_span_id: Option<&str>,
        service: &str,
        name: &str,
        kind: &str,
        attributes: Value,
        typed_links: Vec<Value>,
    ) -> Value {
        json!({
            "spanId": span_id,
            "parentSpanId": parent_span_id,
            "name": name,
            "service": service,
            "kind": kind,
            "statusCode": "UNSET",
            "attributes": attributes.to_string(),
            "resource": "{}",
            "typedLinks": typed_links
        })
    }

    fn complete_traces() -> Vec<Value> {
        let anchor_spans = vec![
            span(
                CHECKOUT_ROOT,
                None,
                "checkout",
                "checkout",
                "SERVER",
                json!({
                    "tenant.id":"tenant-acme",
                    "user.tier":"standard",
                    "customer.segment":"standard",
                    "region":"us-east-1",
                    "request.priority":"normal",
                    "session.id":"session-1"
                }),
                vec![],
            ),
            span(
                CATALOG_SPAN,
                Some(CHECKOUT_ROOT),
                "catalog",
                "HTTP POST",
                "CLIENT",
                json!({}),
                vec![],
            ),
            span(
                PRICING_SPAN,
                Some(CATALOG_SPAN),
                "pricing",
                "pricing.quote",
                "CLIENT",
                json!({}),
                vec![],
            ),
            span(
                INVENTORY_SPAN,
                Some(PRICING_SPAN),
                "inventory",
                "inventory.reserve",
                "CLIENT",
                json!({}),
                vec![],
            ),
            span(
                PAYMENT_SPAN,
                Some(INVENTORY_SPAN),
                "payment",
                "Payment/Authorize",
                "CLIENT",
                json!({}),
                vec![],
            ),
            span(
                CHECKOUT_PRODUCER,
                Some(CHECKOUT_ROOT),
                "checkout",
                "checkout.outbox.publish",
                "SPAN_KIND_PRODUCER",
                json!({
                    "tenant.id":"tenant-acme",
                    "user.tier":"standard",
                    "customer.segment":"standard",
                    "region":"us-east-1",
                    "request.priority":"normal",
                    "session.id":"session-1",
                    "messaging.system":"rabbitmq",
                    "messaging.destination.name":"commerce.events",
                    "messaging.operation.name":"send",
                    "messaging.message.id":"event-1",
                    "commerce.event.type":"order.paid"
                }),
                vec![],
            ),
        ];
        let fulfillment_spans = vec![
            span(
                ORDER_CONSUMER,
                None,
                "fulfillment",
                "order.consumer",
                "SPAN_KIND_CONSUMER",
                json!({
                    "tenant.id":"tenant-acme",
                    "user.tier":"standard",
                    "customer.segment":"standard",
                    "region":"us-east-1",
                    "request.priority":"normal",
                    "session.id":"session-1",
                    "messaging.system":"rabbitmq",
                    "messaging.destination.name":"fulfillment.orders",
                    "messaging.operation.name":"process",
                    "messaging.message.id":"event-1",
                    "commerce.event.type":"order.paid",
                    "order.id":"order-1"
                }),
                vec![link(TRACE_ID, CHECKOUT_PRODUCER)],
            ),
            span(
                NOTIFICATION_SPAN,
                Some(ORDER_CONSUMER),
                "notifications",
                "HTTP POST",
                "CLIENT",
                json!({
                    "tenant.id":"tenant-acme",
                    "user.tier":"standard",
                    "customer.segment":"standard",
                    "region":"us-east-1",
                    "request.priority":"normal",
                    "session.id":"session-1"
                }),
                vec![],
            ),
            span(
                FULFILLMENT_PRODUCER,
                Some(ORDER_CONSUMER),
                "fulfillment",
                "analytics.publish",
                "SPAN_KIND_PRODUCER",
                json!({
                    "tenant.id":"tenant-acme",
                    "user.tier":"standard",
                    "customer.segment":"standard",
                    "region":"us-east-1",
                    "request.priority":"normal",
                    "session.id":"session-1",
                    "messaging.system":"rabbitmq",
                    "messaging.destination.name":"commerce.events",
                    "messaging.operation.name":"send",
                    "messaging.message.id":"event-1"
                }),
                vec![],
            ),
        ];
        let analytics_spans = vec![span(
            ANALYTICS_CONSUMER,
            None,
            "fulfillment",
            "analytics.consumer",
            "SPAN_KIND_CONSUMER",
            json!({
                "tenant.id":"tenant-acme",
                "user.tier":"standard",
                "customer.segment":"standard",
                "region":"us-east-1",
                "request.priority":"normal",
                "session.id":"session-1",
                "messaging.system":"rabbitmq",
                "messaging.destination.name":"analytics.events",
                "messaging.operation.name":"process",
                "commerce.event.type":"order.paid",
                "messaging.message.id":"event-1"
            }),
            vec![link(TRACE_ID, CHECKOUT_PRODUCER)],
        )];
        vec![
            json!({"traceId": TRACE_ID, "spans": anchor_spans}),
            json!({"traceId": FULFILLMENT_TRACE_ID, "spans": fulfillment_spans}),
            json!({"traceId": ANALYTICS_TRACE_ID, "spans": analytics_spans}),
        ]
    }

    fn browser_traces() -> Vec<Value> {
        let mut traces = complete_traces();
        traces[0]["spans"][0]["parentSpanId"] = json!(STOREFRONT_SPAN);
        let storefront = span(
            STOREFRONT_SPAN,
            Some(WEB_SPAN),
            "storefront",
            "GraphQL checkout",
            "SERVER",
            json!({
                "tenant.id":"tenant-acme",
                "user.tier":"standard",
                "customer.segment":"standard",
                "region":"us-east-1",
                "request.priority":"normal",
                "session.id":"session-1"
            }),
            vec![],
        );
        let web = span(
            WEB_SPAN,
            None,
            "web",
            "HTTP POST /__storefront/graphql",
            "SERVER",
            json!({
                "tenant.id":"tenant-acme",
                "user.tier":"standard",
                "customer.segment":"standard",
                "region":"us-east-1",
                "request.priority":"normal",
                "session.id":"session-1"
            }),
            vec![],
        );
        let anchor_spans = traces[0]["spans"].as_array_mut().expect("anchor spans");
        anchor_spans.insert(0, storefront);
        anchor_spans.insert(0, web);
        traces
    }

    fn clickhouse_event() -> Value {
        let traceparent = format!("00-{TRACE_ID}-0123456789abcdef-01");
        let tracestate = "playground=commerce";
        let baggage = BUSINESS_BAGGAGE;
        json!({
            "eventId": "bf442986-ff20-5ff4-b3bf-ed2ec1246160",
            "tenantId": "tenant-acme",
            "eventKey": "order-1:paid",
            "eventName": "order.paid",
            "source": "fulfillment",
            "entityType": "order",
            "entityId": "order-1",
            "traceId": TRACE_ID,
            "spanId": "0123456789abcdef",
            "traceparent": traceparent,
            "tracestate": tracestate,
            "baggage": baggage,
            "properties": "{\"total_minor\":2199}",
            "context": json!({
                "traceparent": traceparent,
                "tracestate": tracestate,
                "baggage": baggage,
                "baggage_items": {
                    "tenant.id":"tenant-acme",
                    "user.tier":"standard",
                    "customer.segment":"standard",
                    "region":"us-east-1",
                    "request.priority":"normal",
                    "session.id":"session-1"
                }
            }).to_string()
        })
    }

    fn update_span_attributes(
        traces: &mut [Value],
        trace_index: usize,
        span_index: usize,
        update: impl FnOnce(&mut serde_json::Map<String, Value>),
    ) {
        let raw = traces[trace_index]["spans"][span_index]["attributes"]
            .as_str()
            .expect("span attributes")
            .to_owned();
        let mut attributes = serde_json::from_str::<Value>(&raw).expect("span attributes JSON");
        update(attributes.as_object_mut().expect("span attributes object"));
        traces[trace_index]["spans"][span_index]["attributes"] = json!(attributes.to_string());
    }

    fn update_link_attributes(
        traces: &mut [Value],
        trace_index: usize,
        span_index: usize,
        link_index: usize,
        update: impl FnOnce(&mut serde_json::Map<String, Value>),
    ) {
        let raw = traces[trace_index]["spans"][span_index]["typedLinks"][link_index]["attributes"]
            .as_str()
            .expect("link attributes")
            .to_owned();
        let mut attributes = serde_json::from_str::<Value>(&raw).expect("link attributes JSON");
        update(attributes.as_object_mut().expect("link attributes object"));
        traces[trace_index]["spans"][span_index]["typedLinks"][link_index]["attributes"] =
            json!(attributes.to_string());
    }

    #[test]
    fn complete_topology_passes() -> Result<()> {
        let summary = analyze(TRACE_ID, &complete_traces())?;
        assert_eq!(summary.traces, 3);
        assert_eq!(summary.spans, 10);
        assert_eq!(
            summary.services,
            vec![
                "catalog",
                "checkout",
                "fulfillment",
                "inventory",
                "notifications",
                "payment",
                "pricing"
            ]
        );
        Ok(())
    }

    #[test]
    fn complete_fixture_has_unique_nonempty_span_identity_and_edges() {
        let traces = complete_traces();
        assert_eq!(traces.len(), 3);
        let mut identities = HashSet::new();
        let mut span_count = 0;
        let mut parent_edges = 0;
        let mut typed_links = 0;
        for trace in &traces {
            let trace_id = trace["traceId"].as_str().expect("fixture trace id");
            assert!(is_trace_id(trace_id));
            let spans = trace["spans"].as_array().expect("fixture spans");
            assert!(!spans.is_empty());
            for span in spans {
                let span_id = span["spanId"].as_str().expect("fixture span id");
                assert!(is_span_id(span_id));
                assert!(identities.insert((trace_id, span_id)));
                assert!(span["name"].as_str().is_some_and(|value| !value.is_empty()));
                assert!(
                    span["service"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                );
                assert!(
                    span["attributes"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty())
                );
                if span["parentSpanId"].as_str().is_some() {
                    parent_edges += 1;
                }
                let links = span["typedLinks"].as_array().expect("fixture links");
                for link in links {
                    assert!(is_trace_id(
                        link["traceId"].as_str().expect("link trace id")
                    ));
                    assert!(is_span_id(link["spanId"].as_str().expect("link span id")));
                    assert!(
                        link["attributes"]
                            .as_str()
                            .is_some_and(|value| !value.is_empty())
                    );
                }
                span_count += 1;
                typed_links += links.len();
            }
        }
        assert_eq!(span_count, 10);
        assert_eq!(identities.len(), 10);
        assert!(parent_edges > 0);
        assert_eq!(typed_links, 2);
    }

    #[test]
    fn checkout_root_requires_every_business_baggage_key() {
        for key in REQUIRED_BUSINESS_BAGGAGE_KEYS {
            let mut traces = complete_traces();
            update_span_attributes(&mut traces, 0, 0, |attributes| {
                attributes.remove(key);
            });
            assert!(
                analyze(TRACE_ID, &traces).is_err(),
                "missing {key} must fail closed"
            );
        }
    }

    #[test]
    fn checkout_rabbit_edges_require_each_w3c_carrier_field() {
        for (trace_index, label) in [(1, "fulfillment"), (2, "analytics")] {
            for field in ["traceparent", "tracestate", "baggage"] {
                let mut traces = complete_traces();
                update_link_attributes(&mut traces, trace_index, 0, 0, |attributes| {
                    attributes.remove(field);
                });
                assert!(
                    analyze(TRACE_ID, &traces).is_err(),
                    "missing {field} on {label} edge must fail closed"
                );
            }
        }
    }

    #[test]
    fn checkout_rabbit_edge_traceparent_must_identify_checkout_producer() {
        let mut traces = complete_traces();
        update_link_attributes(&mut traces, 1, 0, 0, |attributes| {
            attributes.insert(
                "traceparent".to_owned(),
                json!(format!("00-{TRACE_ID}-{ORDER_CONSUMER}-01")),
            );
        });
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn notification_service_presence_without_verified_tenant_evidence_fails() {
        let mut traces = complete_traces();
        update_span_attributes(&mut traces, 1, 1, |attributes| {
            attributes.insert("tenant.id".to_owned(), json!("tenant-other"));
        });
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn notification_requires_an_order_consumer_ancestor() -> Result<()> {
        let mut traces = complete_traces();
        traces[1]["spans"][1]["parentSpanId"] = Value::Null;
        let nodes = collect_span_nodes(TRACE_ID, &traces)?;
        let index = index_spans(&nodes)?;
        let order_consumers = matching_span_indices(
            &nodes,
            "fulfillment",
            &[
                ("messaging.system", "rabbitmq"),
                ("messaging.destination.name", "fulfillment.orders"),
                ("messaging.operation.name", "process"),
                ("commerce.event.type", "order.paid"),
            ],
        );
        let business_baggage = BusinessBaggage::from_attribute_object(
            &span_attribute_object(nodes[0].span)?,
            "checkout business baggage",
        )?;
        let identity = CommerceIdentity {
            tenant_id: "tenant-acme".to_owned(),
            event_id: "event-1".to_owned(),
            event_type: "order.paid".to_owned(),
            event_key: "order-1:paid".to_owned(),
            order_id: "order-1".to_owned(),
        };
        assert!(
            require_notification_evidence(
                &nodes,
                &index,
                &order_consumers,
                &business_baggage,
                &identity,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn applicable_session_id_must_survive_business_baggage_propagation() {
        let mut traces = complete_traces();
        update_span_attributes(&mut traces, 2, 0, |attributes| {
            attributes.remove("session.id");
        });
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn analytics_name_without_clickhouse_consumer_contract_fails() {
        let mut traces = complete_traces();
        let analytics = traces
            .iter_mut()
            .flat_map(|trace| trace["spans"].as_array_mut().into_iter().flatten())
            .find(|span| span.get("name").and_then(Value::as_str) == Some("analytics.consumer"))
            .expect("analytics fixture");
        analytics["attributes"] = json!("{}");
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_duplicate_span_identity() {
        let mut traces = complete_traces();
        traces[0]["spans"][1]["spanId"] = json!(CHECKOUT_ROOT);
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_parent_that_is_not_in_the_same_trace() {
        let mut traces = complete_traces();
        traces[0]["spans"][1]["parentSpanId"] = json!("00000000000000ff");
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_empty_parent_id() {
        let mut traces = complete_traces();
        traces[0]["spans"][1]["parentSpanId"] = json!("");
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_cyclic_parent_chain() {
        let mut traces = complete_traces();
        traces[0]["spans"][1]["parentSpanId"] = json!(PAYMENT_SPAN);
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_disconnected_parent_root() {
        let mut traces = complete_traces();
        traces[0]["spans"][4]["parentSpanId"] = Value::Null;
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_disconnected_linked_trace() {
        let mut traces = complete_traces();
        traces[1]["spans"][0]["typedLinks"] = json!([]);
        traces[2]["spans"][0]["typedLinks"] = json!([]);
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_typed_link_to_missing_span() {
        let mut traces = complete_traces();
        traces[1]["spans"][0]["typedLinks"][0]["spanId"] = json!("00000000000000ff");
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_typed_link_with_mismatched_message_identity() {
        let mut traces = complete_traces();
        traces[1]["spans"][0]["attributes"] = json!(
            "{\"tenant.id\":\"tenant-acme\",\"messaging.system\":\"rabbitmq\",\"messaging.destination.name\":\"fulfillment.orders\",\"messaging.operation.name\":\"process\",\"messaging.message.id\":\"event-2\"}"
        );
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_typed_link_without_message_identity() {
        let mut traces = complete_traces();
        traces[1]["spans"][0]["attributes"] = json!(
            "{\"tenant.id\":\"tenant-acme\",\"messaging.system\":\"rabbitmq\",\"messaging.destination.name\":\"fulfillment.orders\",\"messaging.operation.name\":\"process\"}"
        );
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_producer_to_consumer_link() {
        let mut traces = complete_traces();
        traces[0]["spans"][5]["typedLinks"] = json!([link(FULFILLMENT_TRACE_ID, ORDER_CONSUMER)]);
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_reverse_consumer_link_even_when_target_exists() {
        let mut traces = complete_traces();
        traces[1]["spans"][0]["typedLinks"][0] = link(FULFILLMENT_TRACE_ID, FULFILLMENT_PRODUCER);
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_rejects_duplicate_typed_link() {
        let mut traces = complete_traces();
        let duplicate = traces[1]["spans"][0]["typedLinks"][0].clone();
        traces[1]["spans"][0]["typedLinks"]
            .as_array_mut()
            .expect("typed links")
            .push(duplicate);
        assert!(analyze(TRACE_ID, &traces).is_err());
    }

    #[test]
    fn topology_requires_parent_and_link_fields() {
        let mut missing_parent = complete_traces();
        missing_parent[0]["spans"][1]
            .as_object_mut()
            .expect("span object")
            .remove("parentSpanId");
        assert!(analyze(TRACE_ID, &missing_parent).is_err());

        let mut missing_links = complete_traces();
        missing_links[0]["spans"][0]
            .as_object_mut()
            .expect("span object")
            .remove("typedLinks");
        assert!(analyze(TRACE_ID, &missing_links).is_err());
    }

    #[test]
    fn clickhouse_event_preserves_all_w3c_fields() -> Result<()> {
        validate_clickhouse_event(&clickhouse_event(), "tenant-acme", TRACE_ID)
    }

    #[test]
    fn clickhouse_event_rejects_each_missing_w3c_field() {
        for field in ["traceparent", "tracestate", "baggage"] {
            let mut event = clickhouse_event();
            event.as_object_mut().expect("event object").remove(field);
            assert!(
                validate_clickhouse_event(&event, "tenant-acme", TRACE_ID).is_err(),
                "missing {field} must fail closed"
            );
        }
    }

    #[test]
    fn clickhouse_event_rejects_context_propagation_mismatch() {
        let mut event = clickhouse_event();
        event["context"] = json!(
            "{\"traceparent\":\"00-00000000000000000000000000000000-0123456789abcdef-01\",\"tracestate\":\"playground=commerce\",\"baggage\":\"tenant.id=tenant-acme\",\"baggage_items\":{\"tenant.id\":\"tenant-acme\"}}"
        );
        assert!(validate_clickhouse_event(&event, "tenant-acme", TRACE_ID).is_err());
    }

    #[test]
    fn clickhouse_event_rejects_cross_tenant_or_span_evidence() {
        let mut tenant_mismatch = clickhouse_event();
        tenant_mismatch["tenantId"] = json!("tenant-other");
        assert!(validate_clickhouse_event(&tenant_mismatch, "tenant-acme", TRACE_ID).is_err());

        let mut span_mismatch = clickhouse_event();
        span_mismatch["spanId"] = json!("fedcba9876543210");
        assert!(validate_clickhouse_event(&span_mismatch, "tenant-acme", TRACE_ID).is_err());
    }

    #[test]
    fn tracestate_accepts_internal_ascii_spaces() {
        assert!(validate_tracestate("vendor=state with internal spaces").is_ok());
    }

    #[test]
    fn tracestate_accepts_vendor_key_punctuation() {
        assert!(validate_tracestate("a_*/-@b_*/-=state").is_ok());
    }

    #[test]
    fn tracestate_rejects_trailing_space_control_comma_invalid_equals_and_duplicates() {
        assert!(!valid_tracestate_value("state "));
        for value in [
            "vendor=state\n",
            "vendor=state,",
            "vendor=state=extra",
            "vendor=one,vendor=two",
        ] {
            assert!(
                validate_tracestate(value).is_err(),
                "tracestate must reject {value:?}"
            );
        }
    }

    #[test]
    fn tracestate_rejects_oversized_header_and_member_value() {
        let oversized_value = format!("vendor={}", "x".repeat(MAX_TRACESTATE_VALUE_BYTES + 1));
        assert!(validate_tracestate(&oversized_value).is_err());

        let oversized_header = format!(
            "a={},b={},c={}",
            "x".repeat(170),
            "y".repeat(170),
            "z".repeat(170)
        );
        assert!(oversized_header.len() > MAX_TRACESTATE_HEADER_BYTES);
        assert!(validate_tracestate(&oversized_header).is_err());
    }

    #[test]
    fn validates_trace_id() {
        assert!(is_trace_id(TRACE_ID));
        assert!(!is_trace_id("00000000000000000000000000000000"));
        assert!(!is_trace_id("short"));
    }

    #[test]
    fn bounds_diagnostic_output() {
        let output = bounded_output(&"x".repeat(MAX_ERROR_OUTPUT_CHARS + 32));
        assert_eq!(output.chars().count(), MAX_ERROR_OUTPUT_CHARS + 1);
        assert!(output.ends_with('…'));
    }

    #[test]
    fn parses_bounded_w3c_durations() {
        assert_eq!(
            configured_millis("COMMERCE_VERIFY_TEST_DURATION_MILLIS", 1, 10)
                .expect("default duration"),
            Duration::from_millis(1)
        );
    }
}
