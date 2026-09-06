use crate::domain::NotificationRequest;
use anyhow::{Context as _, anyhow};
use axum::http::{HeaderMap, HeaderValue};
use deadpool_postgres::{Config, ManagerConfig, Pool, PoolConfig, RecyclingMethod, Runtime};
use opentelemetry::context::FutureExt as _;
use reqwest::Client;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::watch;
use tokio_postgres::NoTls;
use tracing::Instrument;

pub(crate) const DEFAULT_DATABASE_URL: &str =
    "postgres://postgres:playground@postgres:5432/playground";
pub(crate) const DB_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const MAX_ATTEMPTS: i32 = 4;
const WORKER_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_ERROR_LENGTH: usize = 512;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) pool: Pool,
    pub(crate) http: Client,
    pub(crate) webhook_url: Option<String>,
    pub(crate) email_url: Option<String>,
    pub(crate) worker_ready: Arc<AtomicBool>,
}

impl AppState {
    pub(crate) fn new(pool: Pool) -> anyhow::Result<Self> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .build()
            .context("build notification channel client")?;
        Ok(Self {
            pool,
            http,
            webhook_url: configured_url("NOTIFICATIONS_WEBHOOK_URL"),
            email_url: configured_url("NOTIFICATIONS_EMAIL_URL"),
            worker_ready: Arc::new(AtomicBool::new(false)),
        })
    }
}

fn configured_url(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[derive(Debug)]
pub(crate) struct DeliveryRecord {
    pub(crate) id: String,
    pub(crate) attempts: i32,
    pub(crate) status: String,
    pub(crate) channel: String,
    pub(crate) provider: String,
}

#[derive(Debug)]
pub(crate) enum StoreError {
    Timeout,
    Invalid(String),
    Conflict,
    Database(anyhow::Error),
}

pub(crate) async fn record_delivery(
    state: &AppState,
    request: &NotificationRequest,
    event_key: &str,
    payload: &Value,
    headers: &HeaderMap,
) -> Result<DeliveryRecord, StoreError> {
    playground_telemetry::validate_durable_context(headers)
        .map_err(|error| StoreError::Invalid(format!("invalid W3C propagation: {error}")))?;
    let tenant_id = request
        .tenant_id
        .as_deref()
        .ok_or_else(|| StoreError::Invalid("tenant identity is required".to_owned()))?;
    let client = match tokio::time::timeout(DB_TIMEOUT, state.pool.get()).await {
        Ok(Ok(client)) => client,
        Ok(Err(error)) => return Err(StoreError::Database(error.into())),
        Err(_) => return Err(StoreError::Timeout),
    };
    let id = format!("notification-{}", uuid::Uuid::new_v4());
    let traceparent = header_value(headers, "traceparent");
    let tracestate = header_value(headers, "tracestate");
    let baggage = header_values(headers, "baggage");
    let row = client
        .query_one(
            "INSERT INTO notification_deliveries
                (id, tenant_id, event_key, order_id, channel, status, payload,
                 attempts, next_attempt_at, traceparent, tracestate, baggage)
             VALUES ($1, $2, $3, $4, $5, 'queued', $6::jsonb, 0, CURRENT_TIMESTAMP,
                     $7, $8, $9)
             ON CONFLICT (tenant_id, event_key) DO UPDATE
                 SET updated_at = CURRENT_TIMESTAMP
             RETURNING id, attempts, status, order_id, channel",
            &[
                &id,
                &tenant_id,
                &event_key,
                &request.order_id,
                &request.channel,
                &payload,
                &traceparent,
                &tracestate,
                &baggage,
            ],
        )
        .await
        .map_err(anyhow::Error::from)
        .map_err(StoreError::Database)?;

    let stored_order_id: String = row.get(3);
    let stored_channel: String = row.get(4);
    if stored_order_id != request.order_id || stored_channel != request.channel {
        return Err(StoreError::Conflict);
    }
    Ok(DeliveryRecord {
        id: row.get(0),
        attempts: row.get(1),
        status: row.get(2),
        channel: stored_channel,
        provider: channel_provider(
            &request.channel,
            payload,
            state.webhook_url.as_deref(),
            state.email_url.as_deref(),
        )
        .to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChannelEndpoint {
    url: String,
    provider: &'static str,
}

fn require_channel_endpoint(
    channel: &str,
    payload: &Value,
    webhook_url: Option<&str>,
    email_url: Option<&str>,
) -> anyhow::Result<ChannelEndpoint> {
    resolve_channel_endpoint(channel, payload, webhook_url, email_url)
        .ok_or_else(|| anyhow!("notification channel {channel} has no configured real provider"))
}

fn resolve_channel_endpoint(
    channel: &str,
    payload: &Value,
    webhook_url: Option<&str>,
    email_url: Option<&str>,
) -> Option<ChannelEndpoint> {
    let request_url = payload
        .get("url")
        .or_else(|| payload.get("endpoint"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if let Some(url) = request_url {
        return Some(ChannelEndpoint {
            url,
            provider: "request_endpoint",
        });
    }
    match channel {
        "webhook" => webhook_url
            .filter(|value| !value.trim().is_empty())
            .map(|url| ChannelEndpoint {
                url: url.trim().to_owned(),
                provider: "NOTIFICATIONS_WEBHOOK_URL",
            }),
        "email" => email_url
            .filter(|value| !value.trim().is_empty())
            .map(|url| ChannelEndpoint {
                url: url.trim().to_owned(),
                provider: "NOTIFICATIONS_EMAIL_URL",
            }),
        _ => None,
    }
}

fn channel_provider(
    channel: &str,
    payload: &Value,
    webhook_url: Option<&str>,
    email_url: Option<&str>,
) -> &'static str {
    if channel == "in_app" {
        return "in_app_durable_sink";
    }
    resolve_channel_endpoint(channel, payload, webhook_url, email_url)
        .map(|endpoint| endpoint.provider)
        .unwrap_or("unconfigured")
}

#[derive(Debug)]
struct DeliveryJob {
    id: String,
    tenant_id: String,
    event_key: String,
    order_id: String,
    channel: String,
    payload: Value,
    attempt: i32,
    lease_token: String,
    traceparent: Option<String>,
    tracestate: Option<String>,
    baggage: Option<String>,
}

/// Run the durable delivery worker until the shutdown watch is set.
pub(crate) async fn run_delivery_worker(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    loop {
        if *shutdown.borrow() {
            break;
        }

        match reclaim_exhausted(&state).await {
            Ok(()) => state.worker_ready.store(true, Ordering::Release),
            Err(error) => {
                state.worker_ready.store(false, Ordering::Release);
                tracing::error!(error = %error, "notification worker database poll failed");
                if wait_or_shutdown(&mut shutdown).await {
                    break;
                }
            }
        }

        match claim_next(&state).await {
            Ok(Some(job)) => {
                state.worker_ready.store(true, Ordering::Release);
                process_job(&state, job).await;
            }
            Ok(None) => {
                if wait_or_shutdown(&mut shutdown).await {
                    break;
                }
            }
            Err(error) => {
                state.worker_ready.store(false, Ordering::Release);
                tracing::error!(error = %error, "notification worker claim failed");
                if wait_or_shutdown(&mut shutdown).await {
                    break;
                }
            }
        }
    }
    state.worker_ready.store(false, Ordering::Release);
    tracing::info!("notification delivery worker stopped");
    Ok(())
}

async fn wait_or_shutdown(shutdown: &mut watch::Receiver<bool>) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        changed = shutdown.changed() => {
            changed.is_err() || *shutdown.borrow()
        }
        _ = tokio::time::sleep(WORKER_POLL_INTERVAL) => false
    }
}

async fn claim_next(state: &AppState) -> anyhow::Result<Option<DeliveryJob>> {
    let mut client = db_client(&state.pool).await?;
    let transaction = client
        .transaction()
        .await
        .context("start notification claim transaction")?;
    let lease_token = uuid::Uuid::new_v4().to_string();
    let row = transaction
        .query_opt(
            "WITH candidate AS (
                 SELECT id
                   FROM notification_deliveries
                  WHERE attempts < $1
                    AND (
                        (status IN ('accepted', 'queued')
                         AND next_attempt_at <= CURRENT_TIMESTAMP)
                        OR
                        (status = 'processing'
                         AND lease_until <= CURRENT_TIMESTAMP)
                    )
                  ORDER BY next_attempt_at, created_at, id
                  FOR UPDATE SKIP LOCKED
                  LIMIT 1
             )
             UPDATE notification_deliveries AS delivery
                SET status = 'processing',
                    attempts = delivery.attempts + 1,
                    lease_until = CURRENT_TIMESTAMP + INTERVAL '30 seconds',
                    lease_token = $2,
                    updated_at = CURRENT_TIMESTAMP
               FROM candidate
              WHERE delivery.id = candidate.id
             RETURNING delivery.id, delivery.tenant_id, delivery.event_key,
                       delivery.order_id, delivery.channel, delivery.payload,
                       delivery.attempts, delivery.lease_token,
                       delivery.traceparent, delivery.tracestate, delivery.baggage",
            &[&MAX_ATTEMPTS, &lease_token],
        )
        .await
        .context("claim notification delivery")?;
    let Some(row) = row else {
        transaction
            .commit()
            .await
            .context("commit empty notification claim")?;
        return Ok(None);
    };
    let job = DeliveryJob {
        id: row.get(0),
        tenant_id: row.get(1),
        event_key: row.get(2),
        order_id: row.get(3),
        channel: row.get(4),
        payload: row.get(5),
        attempt: row.get(6),
        lease_token: row.get(7),
        traceparent: row.get(8),
        tracestate: row.get(9),
        baggage: row.get(10),
    };
    transaction
        .execute(
            "INSERT INTO notification_delivery_attempts
                (delivery_id, tenant_id, attempt, outcome)
             VALUES ($1, $2, $3, 'processing')
             ON CONFLICT (delivery_id, attempt) DO UPDATE
                 SET outcome = 'processing', error = NULL,
                     started_at = CURRENT_TIMESTAMP, completed_at = NULL",
            &[&job.id, &job.tenant_id, &job.attempt],
        )
        .await
        .context("record notification delivery attempt")?;
    transaction
        .commit()
        .await
        .context("commit notification claim")?;
    Ok(Some(job))
}

async fn reclaim_exhausted(state: &AppState) -> anyhow::Result<()> {
    let client = db_client(&state.pool).await?;
    let rows = client
        .query(
            "UPDATE notification_deliveries
                SET status = 'dead_lettered',
                    dead_lettered_at = COALESCE(dead_lettered_at, CURRENT_TIMESTAMP),
                    last_error = COALESCE(last_error, 'delivery retry budget exhausted'),
                    lease_until = NULL,
                    lease_token = NULL,
                    updated_at = CURRENT_TIMESTAMP
              WHERE attempts >= $1
                AND (
                    status IN ('accepted', 'queued')
                    OR (status = 'processing' AND lease_until <= CURRENT_TIMESTAMP)
                )
             RETURNING id, tenant_id, attempts",
            &[&MAX_ATTEMPTS],
        )
        .await
        .context("dead-letter exhausted notification deliveries")?;
    for row in rows {
        let id: String = row.get(0);
        let tenant_id: String = row.get(1);
        let attempt: i32 = row.get(2);
        client
            .execute(
                "INSERT INTO notification_delivery_attempts
                    (delivery_id, tenant_id, attempt, outcome, error, completed_at)
                 VALUES ($1, $2, $3, 'dead_lettered',
                         'delivery retry budget exhausted', CURRENT_TIMESTAMP)
                 ON CONFLICT (delivery_id, attempt) DO UPDATE
                     SET outcome = 'dead_lettered',
                         error = EXCLUDED.error,
                         completed_at = CURRENT_TIMESTAMP",
                &[&id, &tenant_id, &attempt],
            )
            .await
            .context("record exhausted notification delivery")?;
    }
    Ok(())
}

async fn process_job(state: &AppState, job: DeliveryJob) {
    let provider = channel_provider(
        &job.channel,
        &job.payload,
        state.webhook_url.as_deref(),
        state.email_url.as_deref(),
    );
    let headers = match job_context_headers(&job) {
        Ok(headers) => headers,
        Err(error) => {
            tracing::error!(
                error = %error,
                delivery_id = %job.id,
                "invalid durable W3C propagation; notification quarantined"
            );
            if let Err(mark_error) = dead_letter_invalid_propagation(state, &job, &error).await {
                tracing::error!(
                    error = %mark_error,
                    delivery_id = %job.id,
                    "invalid notification propagation quarantine failed"
                );
            }
            return;
        }
    };
    let span = tracing::info_span!(
        "notifications.delivery",
        otel.kind = playground_telemetry::semconv::SPAN_KIND_CONSUMER,
        tenant.id = %job.tenant_id,
        job.id = %job.id,
        messaging.message.id = %job.event_key,
        messaging.delivery.attempt = job.attempt,
        notification.channel = %job.channel,
        notification.provider = %provider,
    );
    playground_telemetry::set_parent_from_headers(&span, &headers);
    let parent = playground_telemetry::extract_context(&headers);
    playground_telemetry::stamp_business_baggage(&span, &parent);
    let result = dispatch_notification(state, &job)
        .instrument(span.clone())
        .with_context(parent)
        .await;
    match result {
        Ok(receipt) => {
            tracing::info!(
                delivery_id = %job.id,
                channel = %receipt.channel,
                provider = %receipt.provider,
                acknowledgement_reference = %receipt.acknowledgement_reference,
                "notification provider acknowledged delivery"
            );
            if let Err(error) = mark_delivered(state, &job).await {
                tracing::error!(error = %error, delivery_id = %job.id, "notification acknowledgement persistence failed");
            }
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                delivery_id = %job.id,
                attempt = job.attempt,
                "notification channel dispatch failed"
            );
            if let Err(mark_error) =
                mark_failed_or_dead_letter(state, &job, &error.to_string()).await
            {
                tracing::error!(error = %mark_error, delivery_id = %job.id, "notification retry state persistence failed");
            }
        }
    }
}

#[derive(Debug)]
struct DispatchReceipt {
    channel: String,
    provider: &'static str,
    acknowledgement_reference: String,
}

async fn dispatch_notification(
    state: &AppState,
    job: &DeliveryJob,
) -> anyhow::Result<DispatchReceipt> {
    match job.channel.as_str() {
        "in_app" => {
            persist_channel_message(state, job, "in-app")
                .await
                .context("acknowledge in-app channel")?;
            Ok(DispatchReceipt {
                channel: job.channel.clone(),
                provider: "in_app_durable_sink",
                acknowledgement_reference: "in-app".to_owned(),
            })
        }
        "webhook" | "email" => {
            let endpoint = require_channel_endpoint(
                &job.channel,
                &job.payload,
                state.webhook_url.as_deref(),
                state.email_url.as_deref(),
            )?;
            let mut headers = HeaderMap::new();
            playground_telemetry::inject_headers(&mut headers);
            headers.insert(
                "x-tenant-id",
                HeaderValue::try_from(job.tenant_id.as_str())
                    .context("encode notification tenant header")?,
            );
            headers.insert(
                "idempotency-key",
                HeaderValue::try_from(job.id.as_str())
                    .context("encode notification idempotency header")?,
            );
            let body = json!({
                "delivery_id": job.id,
                "event_key": job.event_key,
                "tenant_id": job.tenant_id,
                "order_id": job.order_id,
                "channel": job.channel,
                "payload": job.payload,
            });
            let response = state
                .http
                .post(&endpoint.url)
                .headers(headers)
                .json(&body)
                .send()
                .await
                .context("dispatch notification channel request")?;
            let status = response.status();
            if !status.is_success() {
                return Err(anyhow!("notification channel returned HTTP {status}"));
            }
            let acknowledgement = response
                .headers()
                .get("x-ack-id")
                .and_then(|value| value.to_str().ok())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("http-{status}"));
            persist_channel_message(state, job, &acknowledgement)
                .await
                .context("persist external notification acknowledgement")?;
            Ok(DispatchReceipt {
                channel: job.channel.clone(),
                provider: endpoint.provider,
                acknowledgement_reference: acknowledgement,
            })
        }
        channel => Err(anyhow!("unsupported notification channel {channel}")),
    }
}

async fn persist_channel_message(
    state: &AppState,
    job: &DeliveryJob,
    acknowledgement: &str,
) -> anyhow::Result<()> {
    let client = db_client(&state.pool).await?;
    client
        .execute(
            "INSERT INTO notification_channel_messages
                (delivery_id, tenant_id, order_id, channel, payload,
                 acknowledged_at, acknowledgement_reference)
             VALUES ($1, $2, $3, $4, $5::jsonb, CURRENT_TIMESTAMP, $6)
             ON CONFLICT (delivery_id) DO UPDATE
                 SET acknowledged_at = notification_channel_messages.acknowledged_at",
            &[
                &job.id,
                &job.tenant_id,
                &job.order_id,
                &job.channel,
                &job.payload,
                &acknowledgement,
            ],
        )
        .await
        .context("persist notification channel message")?;
    Ok(())
}

async fn mark_delivered(state: &AppState, job: &DeliveryJob) -> anyhow::Result<()> {
    let mut client = db_client(&state.pool).await?;
    let transaction = client
        .transaction()
        .await
        .context("start notification completion transaction")?;
    let updated = transaction
        .execute(
            "UPDATE notification_deliveries
                SET status = 'delivered',
                    delivered_at = CURRENT_TIMESTAMP,
                    acknowledged_at = CURRENT_TIMESTAMP,
                    lease_until = NULL,
                    lease_token = NULL,
                    last_error = NULL,
                    updated_at = CURRENT_TIMESTAMP
              WHERE id = $1 AND tenant_id = $2 AND status = 'processing'
                AND lease_token = $3",
            &[&job.id, &job.tenant_id, &job.lease_token],
        )
        .await
        .context("mark notification delivered")?;
    if updated != 1 {
        return Err(anyhow!(
            "notification delivery lease was lost before acknowledgement"
        ));
    }
    transaction
        .execute(
            "UPDATE notification_delivery_attempts
                SET outcome = 'delivered', completed_at = CURRENT_TIMESTAMP, error = NULL
              WHERE delivery_id = $1 AND tenant_id = $2 AND attempt = $3",
            &[&job.id, &job.tenant_id, &job.attempt],
        )
        .await
        .context("mark notification attempt delivered")?;
    transaction
        .commit()
        .await
        .context("commit notification completion")?;
    Ok(())
}

async fn mark_failed_or_dead_letter(
    state: &AppState,
    job: &DeliveryJob,
    error: &str,
) -> anyhow::Result<()> {
    let mut client = db_client(&state.pool).await?;
    let transaction = client
        .transaction()
        .await
        .context("start notification retry transaction")?;
    let terminal = job.attempt >= MAX_ATTEMPTS;
    let stored_error = truncate_error(error);
    let next_attempt = backoff(job.attempt);
    let outcome = if terminal { "dead_lettered" } else { "retry" };
    let updated = if terminal {
        transaction
            .execute(
                "UPDATE notification_deliveries
                    SET status = 'dead_lettered',
                        dead_lettered_at = CURRENT_TIMESTAMP,
                        lease_until = NULL,
                        lease_token = NULL,
                        last_error = $4,
                        updated_at = CURRENT_TIMESTAMP
                  WHERE id = $1 AND tenant_id = $2 AND status = 'processing'
                    AND lease_token = $3",
                &[&job.id, &job.tenant_id, &job.lease_token, &stored_error],
            )
            .await
            .context("dead-letter notification delivery")?
    } else {
        transaction
            .execute(
                "UPDATE notification_deliveries
                    SET status = 'queued',
                        next_attempt_at = CURRENT_TIMESTAMP + $4 * INTERVAL '1 second',
                        lease_until = NULL,
                        lease_token = NULL,
                        last_error = $5,
                        updated_at = CURRENT_TIMESTAMP
                  WHERE id = $1 AND tenant_id = $2 AND status = 'processing'
                    AND lease_token = $3",
                &[
                    &job.id,
                    &job.tenant_id,
                    &job.lease_token,
                    &next_attempt,
                    &stored_error,
                ],
            )
            .await
            .context("schedule notification retry")?
    };
    if updated != 1 {
        return Err(anyhow!(
            "notification delivery lease was lost before retry state update"
        ));
    }
    transaction
        .execute(
            "UPDATE notification_delivery_attempts
                SET outcome = $4, error = $5, completed_at = CURRENT_TIMESTAMP
              WHERE delivery_id = $1 AND tenant_id = $2 AND attempt = $3",
            &[
                &job.id,
                &job.tenant_id,
                &job.attempt,
                &outcome,
                &stored_error,
            ],
        )
        .await
        .context("record notification retry outcome")?;
    transaction
        .commit()
        .await
        .context("commit notification retry")?;
    Ok(())
}

async fn dead_letter_invalid_propagation(
    state: &AppState,
    job: &DeliveryJob,
    error: &anyhow::Error,
) -> anyhow::Result<()> {
    let mut client = db_client(&state.pool).await?;
    let transaction = client
        .transaction()
        .await
        .context("start invalid notification quarantine transaction")?;
    let stored_error = truncate_error(&format!("invalid W3C propagation: {error}"));
    let updated = transaction
        .execute(
            "UPDATE notification_deliveries
                SET status = 'dead_lettered',
                    dead_lettered_at = CURRENT_TIMESTAMP,
                    lease_until = NULL,
                    lease_token = NULL,
                    last_error = $4,
                    updated_at = CURRENT_TIMESTAMP
              WHERE id = $1 AND tenant_id = $2 AND status = 'processing'
                AND lease_token = $3",
            &[&job.id, &job.tenant_id, &job.lease_token, &stored_error],
        )
        .await
        .context("quarantine invalid notification propagation")?;
    if updated != 1 {
        return Err(anyhow!(
            "notification delivery lease was lost before propagation quarantine"
        ));
    }
    transaction
        .execute(
            "UPDATE notification_delivery_attempts
                SET outcome = 'dead_lettered', error = $4, completed_at = CURRENT_TIMESTAMP
              WHERE delivery_id = $1 AND tenant_id = $2 AND attempt = $3",
            &[&job.id, &job.tenant_id, &job.attempt, &stored_error],
        )
        .await
        .context("record invalid notification propagation")?;
    transaction
        .commit()
        .await
        .context("commit invalid notification quarantine")?;
    Ok(())
}

fn backoff(attempt: i32) -> i32 {
    2_i32
        .saturating_pow(attempt.saturating_sub(1).clamp(0, 4) as u32)
        .min(30)
}

fn truncate_error(error: &str) -> String {
    error.chars().take(MAX_ERROR_LENGTH).collect()
}

fn job_context_headers(job: &DeliveryJob) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    insert_header(&mut headers, "traceparent", job.traceparent.as_deref())?;
    insert_header(&mut headers, "tracestate", job.tracestate.as_deref())?;
    insert_header(&mut headers, "baggage", job.baggage.as_deref())?;
    playground_telemetry::validate_durable_context(&headers)?;
    Ok(headers)
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: Option<&str>,
) -> anyhow::Result<()> {
    let value = value.ok_or_else(|| anyhow!("durable W3C {name} header is missing"))?;
    let value = anyhow::Context::with_context(HeaderValue::try_from(value), || {
        format!("durable W3C {name} header is invalid")
    })?;
    headers.insert(name, value);
    Ok(())
}

fn header_value(headers: &HeaderMap, name: &'static str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn header_values(headers: &HeaderMap, name: &'static str) -> Option<String> {
    let values = headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.join(","))
}

async fn db_client(pool: &Pool) -> anyhow::Result<deadpool_postgres::Object> {
    tokio::time::timeout(DB_TIMEOUT, pool.get())
        .await
        .context("notifications postgres pool timeout")?
        .context("notifications postgres pool unavailable")
}

pub(crate) fn postgres_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut config = Config::new();
    config.url = Some(database_url.to_owned());
    config.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    config.pool = Some(PoolConfig {
        max_size: 8,
        ..Default::default()
    });
    config
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .context("create notifications postgres pool")
}

pub(crate) async fn init_db() -> anyhow::Result<Pool> {
    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
    let pool = postgres_pool(&url)?;
    let client = db_client(&pool).await?;
    client
        .query_one("SELECT count(*) FROM notification_deliveries", &[])
        .await
        .context("notifications schema missing; run mise run infra:postgres_migrate")?;
    client
        .query_one("SELECT count(*) FROM notification_delivery_attempts", &[])
        .await
        .context("notification durability migration 013 missing")?;
    client
        .query_one("SELECT count(*) FROM notification_channel_messages", &[])
        .await
        .context("notification channel migration 013 missing")?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::{
        backoff, channel_provider, require_channel_endpoint, resolve_channel_endpoint,
        wait_or_shutdown, watch,
    };
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn retry_backoff_is_bounded_and_increasing() {
        assert_eq!(backoff(1), 1);
        assert_eq!(backoff(2), 2);
        assert_eq!(backoff(3), 4);
        assert_eq!(backoff(4), 8);
        assert_eq!(backoff(99), 16);
    }

    #[test]
    fn external_channel_without_provider_is_not_a_durable_success() {
        let payload = json!({});
        assert_eq!(
            channel_provider("webhook", &payload, None, None),
            "unconfigured"
        );
        assert!(resolve_channel_endpoint("webhook", &payload, None, None).is_none());
        let error = require_channel_endpoint("webhook", &payload, None, None)
            .expect_err("missing provider must fail dispatch");
        assert!(error.to_string().contains("no configured real provider"));
        assert_eq!(
            channel_provider("email", &payload, None, None),
            "unconfigured"
        );
    }

    #[test]
    fn notification_provider_reports_request_or_configured_channel() {
        let request_payload = json!({"endpoint":"http://provider.test/notify"});
        assert_eq!(
            channel_provider("webhook", &request_payload, None, None),
            "request_endpoint"
        );
        assert_eq!(
            channel_provider("webhook", &json!({}), Some("http://webhook.test"), None),
            "NOTIFICATIONS_WEBHOOK_URL"
        );
        assert_eq!(
            channel_provider("in_app", &json!({}), None, None),
            "in_app_durable_sink"
        );
    }

    #[tokio::test]
    async fn delivery_worker_wait_exits_on_shared_cancellation() {
        let (shutdown_sender, mut shutdown_receiver) = watch::channel(false);
        let waiter = tokio::spawn(async move { wait_or_shutdown(&mut shutdown_receiver).await });

        shutdown_sender
            .send(true)
            .expect("test shutdown signal sends");
        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("delivery wait exits after cancellation")
                .expect("delivery wait task joins")
        );
    }
}
