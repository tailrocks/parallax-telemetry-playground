use crate::semconv;
use anyhow::{anyhow, bail};
use http::{HeaderMap, HeaderValue};
use opentelemetry::baggage::{Baggage, BaggageExt};
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::{Status, TraceContextExt};
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use std::collections::BTreeMap;
use std::fmt;
use tonic::metadata::{Ascii, KeyRef, MetadataKey, MetadataMap};
use tracing_opentelemetry::OpenTelemetrySpanExt;

const MAX_BUSINESS_BAGGAGE_VALUE_LENGTH: usize = 128;
const DEFAULT_TRACESTATE: &str = "playground=commerce";
const MAX_TRACEPARENT_HEADER_BYTES: usize = 55;
const MAX_TRACESTATE_HEADER_BYTES: usize = 512;
const MAX_TRACESTATE_MEMBERS: usize = 32;
const MAX_TRACESTATE_MEMBER_BYTES: usize = 256;
const MAX_BAGGAGE_HEADER_BYTES: usize = 8_192;
const MAX_BAGGAGE_MEMBERS: usize = 64;
const SAFE_BUSINESS_BAGGAGE_KEYS: &[&str] = &[
    semconv::TENANT_ID,
    semconv::USER_TIER,
    "customer.segment",
    "region",
    "request.priority",
    "feature.variant",
    "session.id",
    semconv::CLI_INVOCATION_ID,
];

/// The only tenant identity accepted at a service boundary.
///
/// Callers may carry the identity in the request body/query, an explicit
/// tenant header, or W3C baggage. Every present source must agree. There is
/// deliberately no service-local default: a missing identity is a boundary
/// error, not an invitation to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantIdentityError {
    Missing,
    Invalid,
    Conflicting,
}

impl fmt::Display for TenantIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Missing => "tenant identity is required",
            Self::Invalid => "tenant identity is invalid",
            Self::Conflicting => "tenant identity sources conflict",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for TenantIdentityError {}

/// Resolve tenant identity from an HTTP request.
pub fn resolve_http_tenant_identity(
    headers: &HeaderMap,
    request_tenant: Option<&str>,
) -> Result<String, TenantIdentityError> {
    let mut explicit = Vec::new();
    for name in ["x-tenant-id", "tenant-id"] {
        for value in headers.get_all(name).iter() {
            explicit.push(
                value
                    .to_str()
                    .map_err(|_| TenantIdentityError::Invalid)?
                    .to_owned(),
            );
        }
    }
    let baggage = headers
        .get_all("baggage")
        .iter()
        .map(|value| value.to_str().map_err(|_| TenantIdentityError::Invalid))
        .collect::<Result<Vec<_>, _>>()?;
    resolve_tenant_candidates(request_tenant, explicit, baggage)
}

/// Resolve tenant identity from a gRPC request.
pub fn resolve_grpc_tenant_identity(
    metadata: &MetadataMap,
    request_tenant: Option<&str>,
) -> Result<String, TenantIdentityError> {
    let mut explicit = Vec::new();
    for name in ["x-tenant-id", "tenant-id"] {
        for value in metadata.get_all(name).iter() {
            explicit.push(
                value
                    .to_str()
                    .map_err(|_| TenantIdentityError::Invalid)?
                    .to_owned(),
            );
        }
    }
    let baggage = metadata
        .get_all("baggage")
        .iter()
        .map(|value| value.to_str().map_err(|_| TenantIdentityError::Invalid))
        .collect::<Result<Vec<_>, _>>()?;
    resolve_tenant_candidates(request_tenant, explicit, baggage)
}

fn resolve_tenant_candidates<'a>(
    request_tenant: Option<&str>,
    explicit: impl IntoIterator<Item = String>,
    baggage: impl IntoIterator<Item = &'a str>,
) -> Result<String, TenantIdentityError> {
    let mut candidates = Vec::new();
    if let Some(value) = request_tenant {
        candidates.push(normalize_tenant_candidate(value)?);
    }
    for value in explicit {
        candidates.push(normalize_tenant_candidate(&value)?);
    }
    for value in baggage {
        for member in value.split(',') {
            let Some((key, raw_value)) = member.split_once('=') else {
                continue;
            };
            if key.trim() != semconv::TENANT_ID {
                continue;
            }
            let value = raw_value.split(';').next().unwrap_or_default().trim();
            let value = decode_baggage_value(value)?;
            candidates.push(normalize_tenant_candidate(&value)?);
        }
    }

    let Some(first) = candidates.first() else {
        return Err(TenantIdentityError::Missing);
    };
    if candidates.iter().any(|candidate| candidate != first) {
        return Err(TenantIdentityError::Conflicting);
    }
    Ok(first.clone())
}

fn normalize_tenant_candidate(value: &str) -> Result<String, TenantIdentityError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_BUSINESS_BAGGAGE_VALUE_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(TenantIdentityError::Invalid);
    }
    Ok(value.to_owned())
}

fn decode_baggage_value(value: &str) -> Result<String, TenantIdentityError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(TenantIdentityError::Invalid);
        }
        let high = hex_digit(bytes[index + 1]).ok_or(TenantIdentityError::Invalid)?;
        let low = hex_digit(bytes[index + 2]).ok_or(TenantIdentityError::Invalid)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded).map_err(|_| TenantIdentityError::Invalid)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub fn extract_context(headers: &HeaderMap) -> Context {
    let context =
        global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)));
    sanitize_context(&context)
}

/// Validate and extract a complete carrier persisted with durable work.
///
/// Ordinary HTTP requests may start a new trace or omit optional W3C fields.
/// Durable messages cannot: accepting a partial or malformed carrier would
/// silently detach the retry from its originating trace. This boundary keeps
/// that distinction explicit.
pub fn extract_durable_context(headers: &HeaderMap) -> anyhow::Result<Context> {
    validate_durable_context(headers)?;
    Ok(extract_context(headers))
}

pub fn validate_durable_context(headers: &HeaderMap) -> anyhow::Result<()> {
    let traceparent = single_header(headers, "traceparent")?
        .ok_or_else(|| anyhow!("durable W3C carrier is missing traceparent"))?;
    let tracestate = single_header(headers, "tracestate")?
        .ok_or_else(|| anyhow!("durable W3C carrier is missing tracestate"))?;
    let baggage = joined_header(headers, "baggage")?
        .ok_or_else(|| anyhow!("durable W3C carrier is missing baggage"))?;

    validate_traceparent_header(&traceparent)?;
    validate_tracestate_header(&tracestate)?;
    validate_baggage_header(&baggage)?;
    Ok(())
}

fn single_header(headers: &HeaderMap, name: &'static str) -> anyhow::Result<Option<String>> {
    let values = headers
        .get_all(name)
        .iter()
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| anyhow!("durable W3C {name} header is not valid ASCII"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    match values.as_slice() {
        [] => Ok(None),
        [value] if !value.is_empty() => Ok(Some(value.clone())),
        [_] => bail!("durable W3C {name} header is empty"),
        _ => bail!("durable W3C {name} header is duplicated"),
    }
}

fn joined_header(headers: &HeaderMap, name: &'static str) -> anyhow::Result<Option<String>> {
    let values = headers
        .get_all(name)
        .iter()
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| anyhow!("durable W3C {name} header is not valid ASCII"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    match values.as_slice() {
        [] => Ok(None),
        _ if values.iter().any(String::is_empty) => {
            bail!("durable W3C {name} header is empty")
        }
        _ => Ok(Some(values.join(","))),
    }
}

fn validate_traceparent_header(value: &str) -> anyhow::Result<()> {
    let parts = value.split('-').collect::<Vec<_>>();
    if value.len() != MAX_TRACEPARENT_HEADER_BYTES
        || parts.len() != 4
        || parts[0] != "00"
        || !is_lower_hex(parts[1], 32)
        || all_zero_hex(parts[1])
        || !is_lower_hex(parts[2], 16)
        || all_zero_hex(parts[2])
        || !is_lower_hex(parts[3], 2)
    {
        bail!("durable W3C traceparent is invalid")
    }
    Ok(())
}

fn validate_tracestate_header(value: &str) -> anyhow::Result<()> {
    if value.len() > MAX_TRACESTATE_HEADER_BYTES {
        bail!("durable W3C tracestate exceeds {MAX_TRACESTATE_HEADER_BYTES} bytes")
    }
    let mut keys = std::collections::BTreeSet::new();
    let mut member_count = 0;
    for raw_member in value.split(',') {
        let member = trim_ows(raw_member);
        if member.is_empty() {
            bail!("durable W3C tracestate member is invalid")
        }
        member_count += 1;
        if member_count > MAX_TRACESTATE_MEMBERS {
            bail!("durable W3C tracestate has invalid members")
        }
        let Some((key, member_value)) = member.split_once('=') else {
            bail!("durable W3C tracestate member is invalid")
        };
        if member.is_empty()
            || member.len() > MAX_TRACESTATE_MEMBER_BYTES
            || !valid_tracestate_key(key)
            || member_value.is_empty()
            || member_value.len() > MAX_TRACESTATE_MEMBER_BYTES
            || !valid_tracestate_value(member_value)
            || !keys.insert(key)
        {
            bail!("durable W3C tracestate member is invalid")
        }
    }
    if member_count == 0 {
        bail!("durable W3C tracestate has no members")
    }
    Ok(())
}

fn validate_baggage_header(value: &str) -> anyhow::Result<()> {
    if value.is_empty() {
        bail!("durable W3C baggage is empty")
    }
    if value.len() > MAX_BAGGAGE_HEADER_BYTES {
        bail!("durable W3C baggage exceeds {MAX_BAGGAGE_HEADER_BYTES} bytes")
    }
    let mut keys = std::collections::BTreeSet::new();
    let members = value.split(',').collect::<Vec<_>>();
    if members.len() > MAX_BAGGAGE_MEMBERS {
        bail!("durable W3C baggage exceeds {MAX_BAGGAGE_MEMBERS} members")
    }
    for raw_member in members {
        let member = trim_ows(raw_member);
        if member.is_empty() {
            bail!("durable W3C baggage member is invalid")
        }
        let Some((key, raw_value)) = member.split_once('=') else {
            bail!("durable W3C baggage member is invalid")
        };
        let key = trim_ows(key);
        if !valid_baggage_key(key) || !keys.insert(key) {
            bail!("durable W3C baggage member is invalid")
        }

        let (raw_value, raw_properties) = raw_value
            .split_once(';')
            .map_or((raw_value, None), |(value, properties)| {
                (value, Some(properties))
            });
        if !valid_baggage_value(trim_ows(raw_value)) {
            bail!("durable W3C baggage value is invalid")
        }

        let Some(raw_properties) = raw_properties else {
            continue;
        };
        let mut property_keys = std::collections::BTreeSet::new();
        for raw_property in raw_properties.split(';') {
            let property = trim_ows(raw_property);
            if property.is_empty() {
                bail!("durable W3C baggage property is invalid")
            }
            let (property_key, property_value) = property
                .split_once('=')
                .map_or((property, None), |(key, value)| (key, Some(value)));
            let property_key = trim_ows(property_key);
            if !valid_baggage_key(property_key) || !property_keys.insert(property_key) {
                bail!("durable W3C baggage property is invalid")
            }
            if let Some(property_value) = property_value
                && !valid_baggage_value(trim_ows(property_value))
            {
                bail!("durable W3C baggage property value is invalid")
            }
        }
    }
    Ok(())
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn all_zero_hex(value: &str) -> bool {
    value.bytes().all(|byte| byte == b'0')
}

fn trim_ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
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
        && value.bytes().enumerate().all(|(index, byte)| {
            (byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'*' | b'/'))
                && (index > 0 || byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn valid_tracestate_value(value: &str) -> bool {
    !value.ends_with([' ', '\t'])
        && value
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte) && !matches!(byte, b',' | b'='))
}

fn valid_baggage_key(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_baggage_value(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            if index + 2 >= bytes.len()
                || hex_digit(bytes[index + 1]).is_none()
                || hex_digit(bytes[index + 2]).is_none()
            {
                return false;
            }
            index += 3;
            continue;
        }
        if !is_baggage_octet(byte) {
            return false;
        }
        index += 1;
    }
    true
}

fn is_baggage_octet(byte: u8) -> bool {
    matches!(
        byte,
        0x21
            | 0x23..=0x2b
            | 0x2d..=0x3a
            | 0x3c..=0x5b
            | 0x5d..=0x7e
    ) && byte != b'%'
}

/// Retain only bounded, non-sensitive business baggage before crossing a
/// process or messaging boundary.
#[must_use]
pub fn sanitize_context(context: &Context) -> Context {
    with_safe_baggage(context)
}

/// Attach sanitized baggage from an inbound context to a new active span
/// context without dropping baggage metadata.
#[must_use]
pub fn with_safe_parent_baggage(context: &Context, parent: &Context) -> Context {
    let mut baggage = clone_baggage(context);
    for (key, (value, metadata)) in parent.baggage().iter() {
        if is_safe_baggage_entry(key.as_ref(), value.as_ref()) {
            baggage.insert_with_metadata(key.clone(), value.clone(), metadata.clone());
        }
    }
    context.with_baggage(baggage)
}

pub fn set_parent_from(headers: &HeaderMap) {
    set_parent_if_valid(&tracing::Span::current(), extract_context(headers));
}

pub fn set_parent_from_headers(span: &tracing::Span, headers: &HeaderMap) {
    set_parent_if_valid(span, extract_context(headers));
}

/// Copies the A10 business baggage into a server span for backend inspection.
pub fn stamp_business_baggage(span: &tracing::Span, context: &Context) {
    for key in [
        semconv::TENANT_ID,
        semconv::USER_TIER,
        "customer.segment",
        "region",
        "request.priority",
        "feature.variant",
        "session.id",
    ] {
        if let Some(value) = context.baggage().get(key) {
            span.set_attribute(key, value.to_string());
        }
    }
}

pub fn inject_context_headers(context: &Context, headers: &mut HeaderMap) {
    let context = with_safe_baggage(context);
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut HeaderInjector(headers));
    });
    add_default_tracestate_header(headers);
}

/// Build the complete W3C carrier required by durable persistence.
///
/// This is intentionally fallible: a context without a valid traceparent or
/// complete W3C baggage must never become a partially populated durable row.
pub fn inject_durable_context_headers(context: &Context) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    inject_context_headers(context, &mut headers);
    validate_durable_context(&headers)?;
    Ok(headers)
}

pub fn inject_headers(headers: &mut HeaderMap) {
    inject_context_headers(&current_context(), headers);
}

/// Combine the current tracing span with W3C baggage attached to this task.
#[must_use]
pub fn current_context() -> Context {
    let baggage = clone_baggage(&Context::current());
    tracing::Span::current().context().with_baggage(baggage)
}

/// Add baggage entries without discarding values already attached to a context.
#[must_use]
pub fn extend_baggage(context: &Context, entries: impl IntoIterator<Item = KeyValue>) -> Context {
    let mut baggage = clone_baggage(context);
    for entry in entries {
        let key = entry.key.to_string();
        let value = entry.value.to_string();
        if is_safe_baggage_entry(&key, &value) {
            baggage.insert(entry.key, value);
        }
    }
    context.with_baggage(baggage)
}

/// Add the business context used by the A10 propagation scenario.
#[must_use]
pub fn with_business_baggage(context: &Context, tenant: &str, tier: &str) -> Context {
    with_business_context(context, tenant, tier, "standard", "us-east-1", "normal")
}

/// Add the bounded, non-sensitive business context propagated across all
/// synchronous and asynchronous boundaries.
#[must_use]
pub fn with_business_context(
    context: &Context,
    tenant: &str,
    tier: &str,
    segment: &str,
    region: &str,
    priority: &str,
) -> Context {
    extend_baggage(
        context,
        [
            KeyValue::new(semconv::TENANT_ID, tenant.to_owned()),
            KeyValue::new(semconv::USER_TIER, tier.to_owned()),
            KeyValue::new("customer.segment", segment.to_owned()),
            KeyValue::new("region", region.to_owned()),
            KeyValue::new("request.priority", priority.to_owned()),
        ],
    )
}

/// Enrich the current server span while retaining baggage from its inbound
/// W3C parent, including requests that carry baggage without a valid traceparent.
#[must_use]
pub fn with_business_context_from_parent(
    context: &Context,
    parent: &Context,
    tenant: &str,
    tier: &str,
    segment: &str,
    region: &str,
    priority: &str,
) -> Context {
    let mut context = with_safe_parent_baggage(context, parent);
    // The request identity is authoritative at this boundary. Other
    // business fields inherit the sanitized parent when present; defaults
    // must not overwrite a browser or upstream RPC's chosen context.
    context = extend_baggage(
        &context,
        [KeyValue::new(semconv::TENANT_ID, tenant.to_owned())],
    );
    for (key, value) in [
        (semconv::USER_TIER, tier),
        ("customer.segment", segment),
        ("region", region),
        ("request.priority", priority),
    ] {
        if context.baggage().get(key).is_none() {
            context = extend_baggage(&context, [KeyValue::new(key, value.to_owned())]);
        }
    }
    context
}

fn clone_baggage(context: &Context) -> Baggage {
    context
        .baggage()
        .iter()
        .filter_map(|(key, (value, metadata))| {
            if is_safe_baggage_entry(key.as_ref(), value.as_ref()) {
                Some((key.clone(), (value.clone(), metadata.clone())))
            } else {
                None
            }
        })
        .collect()
}

fn is_safe_baggage_entry(key: &str, value: &str) -> bool {
    SAFE_BUSINESS_BAGGAGE_KEYS.contains(&key)
        && value.len() <= MAX_BUSINESS_BAGGAGE_VALUE_LENGTH
        && !value.chars().any(char::is_control)
}

fn with_safe_baggage(context: &Context) -> Context {
    context.with_baggage(clone_baggage(context))
}

pub fn extract_context_from_env() -> Context {
    let carrier = EnvExtractor::from_env();
    extract_env_context(&carrier)
}

fn extract_env_context(carrier: &EnvExtractor) -> Context {
    let context = global::get_text_map_propagator(|propagator| propagator.extract(carrier));
    with_safe_baggage(&context)
}

pub fn set_parent_from_env(span: &tracing::Span) {
    set_parent_if_valid(span, extract_context_from_env());
}

pub fn context_env(context: &Context) -> Vec<(String, String)> {
    let mut carrier = EnvInjector::default();
    let context = with_safe_baggage(context);
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut carrier);
    });
    let mut values = carrier.into_env();
    if has_nonempty_value(&values, "TRACEPARENT") && !has_nonempty_value(&values, "TRACESTATE") {
        values.push(("TRACESTATE".to_owned(), DEFAULT_TRACESTATE.to_owned()));
    }
    values
}

pub fn current_context_env() -> Vec<(String, String)> {
    context_env(&current_context())
}

pub async fn traced_get(url: &str) -> reqwest::Result<reqwest::Response> {
    traced_get_with_context(&current_context(), url).await
}

pub async fn traced_get_with_context(
    context: &Context,
    url: &str,
) -> reqwest::Result<reqwest::Response> {
    let mut headers = HeaderMap::new();
    inject_context_headers(context, &mut headers);
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

#[derive(Default)]
struct EnvInjector(BTreeMap<&'static str, String>);

impl EnvInjector {
    fn into_env(self) -> Vec<(String, String)> {
        self.0
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect()
    }
}

impl Injector for EnvInjector {
    fn set(&mut self, key: &str, value: String) {
        if let Some(key) = env_key(key) {
            self.0.insert(key, value);
        }
    }
}

struct EnvExtractor {
    values: BTreeMap<&'static str, String>,
}

impl EnvExtractor {
    fn from_env() -> Self {
        let mut values = BTreeMap::new();
        for key in ["TRACEPARENT", "TRACESTATE", "BAGGAGE"] {
            if let Ok(value) = std::env::var(key)
                && !value.trim().is_empty()
            {
                values.insert(key, value);
            }
        }
        Self { values }
    }
}

impl Extractor for EnvExtractor {
    fn get(&self, key: &str) -> Option<&str> {
        env_key(key).and_then(|key| self.values.get(key).map(String::as_str))
    }

    fn keys(&self) -> Vec<&str> {
        self.values.keys().copied().collect()
    }
}

fn env_key(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().as_str() {
        "traceparent" => Some("TRACEPARENT"),
        "tracestate" => Some("TRACESTATE"),
        "baggage" => Some("BAGGAGE"),
        _ => None,
    }
}

pub fn set_parent_from_grpc(metadata: &MetadataMap) {
    set_parent_if_valid(&tracing::Span::current(), extract_grpc_context(metadata));
}

pub fn set_parent_from_grpc_metadata(span: &tracing::Span, metadata: &MetadataMap) {
    set_parent_if_valid(span, extract_grpc_context(metadata));
}

pub fn extract_grpc_context(metadata: &MetadataMap) -> Context {
    let context = global::get_text_map_propagator(|propagator| {
        propagator.extract(&MetadataExtractor(metadata))
    });
    with_safe_baggage(&context)
}

pub fn inject_grpc_metadata(metadata: &mut MetadataMap) {
    inject_grpc_metadata_with_context(&current_context(), metadata);
}

pub fn inject_grpc_metadata_with_context(context: &Context, metadata: &mut MetadataMap) {
    let context = with_safe_baggage(context);
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut MetadataInjector(metadata));
    });
    if metadata
        .get("traceparent")
        .is_some_and(|value| !value.is_empty())
        && metadata
            .get("tracestate")
            .is_none_or(|value| value.is_empty())
    {
        metadata.insert(
            "tracestate",
            "playground=commerce".parse().expect("valid tracestate"),
        );
    }
}

fn add_default_tracestate_header(headers: &mut HeaderMap) {
    if headers
        .get("traceparent")
        .is_some_and(|value| !value.is_empty())
        && headers
            .get("tracestate")
            .is_none_or(|value| value.is_empty())
    {
        headers.insert("tracestate", HeaderValue::from_static(DEFAULT_TRACESTATE));
    }
}

fn has_nonempty_value(values: &[(String, String)], key: &str) -> bool {
    values
        .iter()
        .any(|(name, value)| name == key && !value.is_empty())
}

pub fn mark_span_error(error_type: &'static str) {
    let span = tracing::Span::current();
    span.set_status(Status::error(error_type));
    span.set_attribute(semconv::ERROR_TYPE, error_type);
}

fn set_parent_if_valid(span: &tracing::Span, parent: Context) {
    if parent.span().span_context().is_valid() {
        let _ = span.set_parent(parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;
    use opentelemetry::baggage::KeyValueMetadata;
    use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
    use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn propagator_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("propagator lock")
    }

    #[test]
    fn http_headers_round_trip_trace_context() {
        let _guard = propagator_lock();
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
        assert_eq!(
            headers
                .get("tracestate")
                .and_then(|value| value.to_str().ok()),
            Some("playground=commerce")
        );
    }

    #[test]
    fn env_carrier_injects_trace_context_names() {
        let _guard = propagator_lock();
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
        let vars = context_env(&context);

        assert!(vars.iter().any(|(key, _)| key == "TRACEPARENT"));
        assert!(
            vars.iter()
                .any(|(key, value)| key == "TRACESTATE" && value == "playground=commerce")
        );
        assert!(vars.iter().all(|(key, _)| key == &key.to_ascii_uppercase()));
    }

    #[test]
    fn env_carrier_extracts_trace_context() {
        let _guard = propagator_lock();
        global::set_text_map_propagator(TraceContextPropagator::new());
        let trace_id = TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id");
        let carrier = EnvExtractor {
            values: BTreeMap::from([(
                "TRACEPARENT",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
            )]),
        };
        assert_eq!(
            extract_env_context(&carrier)
                .span()
                .span_context()
                .trace_id(),
            trace_id
        );
    }

    #[test]
    fn durable_carrier_requires_all_w3c_fields() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        headers.insert(
            "tracestate",
            HeaderValue::from_static("playground=commerce"),
        );
        headers.insert("baggage", HeaderValue::from_static("tenant.id=tenant-acme"));
        assert!(validate_durable_context(&headers).is_ok());

        headers.remove("baggage");
        assert!(validate_durable_context(&headers).is_err());
    }

    #[test]
    fn durable_carrier_rejects_invalid_traceparent_and_duplicates() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static("invalid"));
        headers.insert(
            "tracestate",
            HeaderValue::from_static("playground=commerce"),
        );
        headers.insert("baggage", HeaderValue::from_static("tenant.id=tenant-acme"));
        assert!(validate_durable_context(&headers).is_err());

        let mut duplicate = HeaderMap::new();
        duplicate.append(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        duplicate.append(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b8-01"),
        );
        duplicate.insert(
            "tracestate",
            HeaderValue::from_static("playground=commerce"),
        );
        duplicate.insert("baggage", HeaderValue::from_static("tenant.id=tenant-acme"));
        assert!(validate_durable_context(&duplicate).is_err());
    }

    #[test]
    fn durable_tracestate_matches_w3c_member_rules() {
        assert!(validate_tracestate_header("vendor/name=state").is_ok());
        for value in [
            "vendor=state,",
            ",vendor=state",
            "vendor=state,,other=value",
        ] {
            assert!(
                validate_tracestate_header(value).is_err(),
                "durable tracestate must reject {value:?}"
            );
        }
    }

    #[test]
    fn durable_baggage_accepts_ows_metadata_encoded_values_and_equals() {
        assert!(validate_baggage_header(
            " tenant.id = Am%C3%A9lie ; trusted = yes; sampled, vendor!key = value=with=equals \t"
        )
        .is_ok());
        assert!(validate_baggage_header("tenant.id=tenant%20acme").is_ok());
    }

    #[test]
    fn durable_baggage_rejects_malformed_members_values_and_duplicates() {
        for value in [
            "tenant.id",
            "tenant.id=tenant-acme,",
            ",tenant.id=tenant-acme",
            "tenant.id=tenant-acme,,region=us-east-1",
            "tenant.id=tenant-acme;",
            "tenant.id=tenant-acme;;property",
            "tenant.id=tenant-acme;=bad",
            "tenant.id=tenant-acme;property value",
            "tenant.id=tenant%2",
            "tenant.id=tenant%GG",
            "tenant.id=tenant%",
            "tenant.id=tenant acme",
            "tenant.id=\"tenant-acme\"",
            "tenant.id=tenant\nacme",
            "tenant.id=one, tenant.id=two",
            "tenant.id=one;meta=a;meta=b",
        ] {
            assert!(
                validate_baggage_header(value).is_err(),
                "durable baggage must reject {value:?}"
            );
        }
    }

    #[test]
    fn durable_baggage_enforces_header_and_member_limits() {
        let oversized = format!("tenant.id={}", "x".repeat(MAX_BAGGAGE_HEADER_BYTES));
        assert!(validate_baggage_header(&oversized).is_err());

        let too_many_members = (0..=MAX_BAGGAGE_MEMBERS)
            .map(|index| format!("key{index}=value"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(validate_baggage_header(&too_many_members).is_err());
    }

    #[test]
    fn generated_durable_carrier_is_complete_or_rejected() {
        let _guard = propagator_lock();
        global::set_text_map_propagator(
            opentelemetry::propagation::TextMapCompositePropagator::new(vec![
                Box::new(TraceContextPropagator::new()),
                Box::new(BaggagePropagator::new()),
            ]),
        );
        let context = Context::new()
            .with_remote_span_context(SpanContext::new(
                TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id"),
                SpanId::from_hex("00f067aa0ba902b7").expect("span id"),
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            ))
            .with_baggage([KeyValue::new("tenant.id", "tenant-acme")]);

        let headers = inject_durable_context_headers(&context).expect("complete carrier");
        assert!(validate_durable_context(&headers).is_ok());
        assert!(headers.contains_key("traceparent"));
        assert!(headers.contains_key("tracestate"));
        assert!(headers.contains_key("baggage"));
        assert!(inject_durable_context_headers(&Context::new()).is_err());
    }

    #[test]
    fn business_baggage_round_trips_through_http_headers() -> Result<(), String> {
        let _guard = propagator_lock();
        global::set_text_map_propagator(
            opentelemetry::propagation::TextMapCompositePropagator::new(vec![
                Box::new(TraceContextPropagator::new()),
                Box::new(BaggagePropagator::new()),
            ]),
        );
        let context = with_business_baggage(&Context::new(), "tenant-a", "pro");
        let mut headers = HeaderMap::new();
        inject_context_headers(&context, &mut headers);
        let baggage = headers
            .get("baggage")
            .and_then(|value| value.to_str().ok())
            .ok_or("baggage header missing")?;
        let extracted = extract_context(&headers);
        let expected_header_members = ["tenant.id=tenant-a", "user.tier=pro"];
        let actual = (
            expected_header_members
                .iter()
                .all(|member| baggage.split(',').any(|actual| actual == *member)),
            extracted
                .baggage()
                .get(semconv::TENANT_ID)
                .map(ToString::to_string),
            extracted
                .baggage()
                .get(semconv::USER_TIER)
                .map(ToString::to_string),
        );
        if actual != (true, Some("tenant-a".to_string()), Some("pro".to_string())) {
            return Err(format!("baggage propagation mismatch: {actual:?}"));
        }
        Ok(())
    }

    #[test]
    fn business_context_preserves_parent_baggage_and_current_span() {
        let current_trace_id =
            TraceId::from_hex("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").expect("current trace id");
        let parent_trace_id =
            TraceId::from_hex("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").expect("parent trace id");
        let current = Context::new()
            .with_remote_span_context(SpanContext::new(
                current_trace_id,
                SpanId::from_hex("aaaaaaaaaaaaaaaa").expect("current span id"),
                TraceFlags::SAMPLED,
                false,
                TraceState::default(),
            ))
            .with_baggage([KeyValue::new("local.value", "kept")]);
        let parent = Context::new()
            .with_remote_span_context(SpanContext::new(
                parent_trace_id,
                SpanId::from_hex("bbbbbbbbbbbbbbbb").expect("parent span id"),
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            ))
            .with_baggage([KeyValueMetadata::new(
                "feature.variant",
                "kept",
                "trusted=true",
            )]);

        let enriched = with_business_context_from_parent(
            &current,
            &parent,
            "tenant-a",
            "pro",
            "standard",
            "us-east-1",
            "normal",
        );

        assert_eq!(enriched.span().span_context().trace_id(), current_trace_id);
        assert_eq!(
            enriched
                .baggage()
                .get("feature.variant")
                .map(ToString::to_string),
            Some("kept".to_owned())
        );
        assert_eq!(
            enriched
                .baggage()
                .get_with_metadata("feature.variant")
                .map(|(_, metadata)| metadata.as_str()),
            Some("trusted=true")
        );
        assert_eq!(
            enriched
                .baggage()
                .get(semconv::TENANT_ID)
                .map(ToString::to_string),
            Some("tenant-a".to_owned())
        );
        assert!(enriched.baggage().get("local.value").is_none());
    }

    #[test]
    fn session_baggage_is_safe_and_round_trips() -> Result<(), String> {
        let _guard = propagator_lock();
        global::set_text_map_propagator(
            opentelemetry::propagation::TextMapCompositePropagator::new(vec![
                Box::new(TraceContextPropagator::new()),
                Box::new(BaggagePropagator::new()),
            ]),
        );
        let context = extend_baggage(&Context::new(), [KeyValue::new("session.id", "session-a")]);
        let mut headers = HeaderMap::new();
        inject_context_headers(&context, &mut headers);
        let extracted = extract_context(&headers);
        assert_eq!(
            extracted
                .baggage()
                .get("session.id")
                .map(ToString::to_string),
            Some("session-a".to_owned())
        );
        Ok(())
    }

    #[test]
    fn propagation_drops_unknown_or_oversized_baggage() {
        let _guard = propagator_lock();
        global::set_text_map_propagator(
            opentelemetry::propagation::TextMapCompositePropagator::new(vec![
                Box::new(TraceContextPropagator::new()),
                Box::new(BaggagePropagator::new()),
            ]),
        );
        let context = Context::new().with_baggage([
            KeyValue::new("secret.token", "do-not-forward"),
            KeyValue::new(semconv::TENANT_ID, "tenant-a"),
            KeyValue::new(
                "customer.segment",
                "x".repeat(MAX_BUSINESS_BAGGAGE_VALUE_LENGTH + 1),
            ),
        ]);
        let mut headers = HeaderMap::new();
        inject_context_headers(&context, &mut headers);
        let baggage = headers
            .get("baggage")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(baggage.contains("tenant.id=tenant-a"));
        assert!(!baggage.contains("secret.token"));
        assert!(!baggage.contains("customer.segment"));
    }

    #[test]
    fn tenant_identity_requires_one_agreeing_source() {
        let _guard = propagator_lock();
        let empty = HeaderMap::new();
        assert_eq!(
            resolve_http_tenant_identity(&empty, None),
            Err(TenantIdentityError::Missing)
        );

        let mut headers = HeaderMap::new();
        headers.insert("x-tenant-id", HeaderValue::from_static("tenant-acme"));
        headers.insert("baggage", HeaderValue::from_static("tenant.id=tenant-acme"));
        assert_eq!(
            resolve_http_tenant_identity(&headers, Some("tenant-acme")),
            Ok("tenant-acme".to_owned())
        );

        assert_eq!(
            resolve_http_tenant_identity(&headers, Some("tenant-nova")),
            Err(TenantIdentityError::Conflicting)
        );
        headers.insert("tenant-id", HeaderValue::from_static("tenant-nova"));
        assert_eq!(
            resolve_http_tenant_identity(&headers, None),
            Err(TenantIdentityError::Conflicting)
        );
    }

    #[test]
    fn grpc_tenant_identity_decodes_baggage_and_rejects_duplicates() {
        let mut metadata = MetadataMap::new();
        metadata.insert("baggage", "tenant.id=tenant%2Dacme".parse().unwrap());
        assert_eq!(
            resolve_grpc_tenant_identity(&metadata, Some("tenant-acme")),
            Ok("tenant-acme".to_owned())
        );
        metadata.append("baggage", "tenant.id=tenant-nova".parse().unwrap());
        assert_eq!(
            resolve_grpc_tenant_identity(&metadata, None),
            Err(TenantIdentityError::Conflicting)
        );
    }
}
