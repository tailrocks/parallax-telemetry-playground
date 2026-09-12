//! PostgreSQL, RabbitMQ, and durable worker adapters.

use crate::application::{
    compensate_payment_with_fence, compensation_operation_id, generation_compensation_task_key,
    inventory_compensation_task_key, load_pending_order, payment_reconciliation_error,
    reconcile_pending_payment, release_inventory,
};
use crate::domain::*;
use anyhow::Context as _;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use deadpool_postgres::{
    Config, GenericClient, ManagerConfig, Pool, PoolConfig, RecyclingMethod, Runtime,
};
use lapin::{
    BasicProperties, Connection, ConnectionProperties, ExchangeKind,
    options::*,
    publisher_confirm::Confirmation,
    types::{AMQPValue, FieldTable},
};
use opentelemetry::trace::TraceContextExt;
use playground_telemetry::semconv;
use serde_json::{Value, json};
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OnceCell, watch};
use tonic::transport::{Channel, Endpoint};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
#[derive(Clone)]

pub(crate) struct AppState {
    pub(crate) pool: Pool,
    pub(crate) http: reqwest::Client,
    pub(crate) catalog_url: String,
    pub(crate) pricing_endpoint: String,
    pub(crate) pricing_channel: Arc<OnceCell<Channel>>,
    pub(crate) payment_endpoint: String,
    pub(crate) payment_channel: Arc<OnceCell<Channel>>,
    pub(crate) inventory_url: String,
    pub(crate) recommendation_url: String,
    pub(crate) rabbit: Arc<RabbitPublisher>,
}

#[derive(Clone)]

pub(crate) struct RabbitPublisher {
    url: String,
    active: Arc<Mutex<Option<ActiveRabbit>>>,
    next_generation: Arc<AtomicU64>,
}

struct ActiveRabbit {
    generation: u64,
    channel: lapin::Channel,
    _connection: Arc<Connection>,
}

struct RabbitChannel {
    channel: lapin::Channel,
    generation: u64,
}

pub(crate) async fn acquire_db(pool: &Pool) -> ApiResult<deadpool_postgres::Client> {
    tokio::time::timeout(DB_TIMEOUT, pool.get())
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "database_timeout",
                "database pool wait timed out",
            )
        })?
        .map_err(db_error)
}

pub(crate) async fn with_checkout_fence<T, F, Fut>(
    state: &AppState,
    fence: &CheckoutFence,
    operation: F,
) -> ApiResult<T>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = ApiResult<T>>,
{
    // The database check is deliberately outside the remote operation.  A
    // row lock held across payment/inventory I/O turns a bounded timeout into
    // pool exhaustion and lets a stale worker block every later claimant.
    assert_checkout_fence(state, fence).await?;
    let (heartbeat_shutdown, heartbeat_receiver) = watch::channel(false);
    let heartbeat = spawn_checkout_lease_heartbeat(state, fence, heartbeat_receiver);
    let result = operation().await;
    stop_checkout_lease_heartbeat(heartbeat_shutdown, heartbeat).await;

    // Always perform the post-operation fence check, including when the
    // remote call returned an error.  A lost response is not evidence that
    // the remote side rolled back; callers have already persisted the
    // applicable compensation/reconciliation intent before risky I/O.
    let fence_result = assert_checkout_fence(state, fence).await;
    match (result, fence_result) {
        (_, Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
    }
}

fn spawn_checkout_lease_heartbeat(
    state: &AppState,
    fence: &CheckoutFence,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let state = state.clone();
    let fence = fence.clone();
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(CHECKOUT_LEASE_HEARTBEAT_INTERVAL) => {}
            }
            if *shutdown.borrow() {
                break;
            }
            if let Err(error) = refresh_checkout_fence(&state, &fence).await {
                tracing::warn!(
                    fence = fence.kind(),
                    error = %error.message,
                    "checkout lease heartbeat failed; post-operation fencing remains authoritative"
                );
            }
        }
    })
}

const HEARTBEAT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

async fn stop_checkout_lease_heartbeat(
    shutdown: watch::Sender<bool>,
    mut heartbeat: tokio::task::JoinHandle<()>,
) {
    let _ = shutdown.send(true);
    match tokio::time::timeout(HEARTBEAT_DRAIN_TIMEOUT, &mut heartbeat).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "checkout lease heartbeat task failed");
        }
        Err(_) => {
            tracing::error!(
                "checkout lease heartbeat did not stop before its drain deadline; aborting"
            );
            heartbeat.abort();
            let _ = (&mut heartbeat).await;
        }
    }
}

const WORKER_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct WorkerHandles {
    workers: Vec<(&'static str, tokio::task::JoinHandle<()>)>,
}

impl WorkerHandles {
    pub(crate) async fn shutdown(mut self) -> anyhow::Result<()> {
        let deadline = Instant::now() + WORKER_DRAIN_TIMEOUT;
        let mut first_error = None;

        for (name, handle) in &mut self.workers {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = if handle.is_finished() {
                Ok((&mut *handle).await)
            } else {
                tokio::time::timeout(remaining, &mut *handle).await
            };
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(worker = name, error = %error, "checkout worker task failed");
                    if first_error.is_none() {
                        first_error = Some(anyhow::anyhow!(
                            "checkout worker {name} task failed: {error}"
                        ));
                    }
                }
                Err(_) => {
                    tracing::error!(
                        worker = name,
                        "checkout worker did not stop before the drain deadline; aborting"
                    );
                    handle.abort();
                    let _ = (&mut *handle).await;
                    if first_error.is_none() {
                        first_error = Some(anyhow::anyhow!(
                            "checkout worker {name} did not stop before the drain deadline"
                        ));
                    }
                }
            }
        }

        first_error.map_or(Ok(()), Err)
    }
}

async fn refresh_checkout_fence(state: &AppState, fence: &CheckoutFence) -> ApiResult<()> {
    match fence {
        CheckoutFence::Attempt(lease) => {
            let client = acquire_db(&state.pool).await?;
            let changed = client
                .execute(
                    "UPDATE checkout_attempts SET updated_at=CURRENT_TIMESTAMP, lease_expires_at=CURRENT_TIMESTAMP + INTERVAL '10 minutes' WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$3 AND status='started' AND lease_expires_at > CURRENT_TIMESTAMP",
                    &[&lease.tenant_id, &lease.request_id, &lease.token],
                )
                .await
                .map_err(db_error)?;
            if changed == 1 {
                Ok(())
            } else {
                Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    format!("{} lease is no longer current", fence.kind()),
                ))
            }
        }
        CheckoutFence::PaymentReconciliation {
            tenant_id,
            request_id,
            parent_token,
            worker_token,
        } => {
            let mut client = acquire_db(&state.pool).await?;
            let transaction = client.transaction().await.map_err(db_error)?;
            lock_checkout_fence(&transaction, fence).await?;
            let changed = transaction
                .execute(
                    "UPDATE checkout_payment_reconciliations SET updated_at=CURRENT_TIMESTAMP, lease_expires_at=CURRENT_TIMESTAMP + INTERVAL '5 minutes' WHERE tenant_id=$1 AND request_id=$2 AND checkout_lease_token=$3 AND lease_token=$4 AND status='processing' AND lease_expires_at > CURRENT_TIMESTAMP",
                    &[tenant_id, request_id, parent_token, worker_token],
                )
                .await
                .map_err(db_error)?;
            if changed == 1 {
                transaction.commit().await.map_err(db_error)
            } else {
                Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    format!("{} lease is no longer current", fence.kind()),
                ))
            }
        }
        CheckoutFence::Compensation {
            tenant_id,
            request_id,
            token,
            task_id,
            claim_token,
        } => {
            let mut client = acquire_db(&state.pool).await?;
            let transaction = client.transaction().await.map_err(db_error)?;
            lock_checkout_fence(&transaction, fence).await?;
            let changed = transaction
                .execute(
                    "UPDATE checkout_compensation_tasks SET updated_at=CURRENT_TIMESTAMP, claim_expires_at=CURRENT_TIMESTAMP + INTERVAL '5 minutes' WHERE tenant_id=$1 AND id=$2 AND claim_token=$3 AND status='processing' AND claim_expires_at > CURRENT_TIMESTAMP AND checkout_request_id=$4 AND checkout_lease_token=$5",
                    &[tenant_id, task_id, claim_token, request_id, token],
                )
                .await
                .map_err(db_error)?;
            if changed != 1 {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    "compensation worker lease is no longer current",
                ));
            }
            transaction
                .execute(
                    "UPDATE checkout_attempts SET updated_at=CURRENT_TIMESTAMP, lease_expires_at=CURRENT_TIMESTAMP + INTERVAL '10 minutes' WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$3 AND status='started' AND lease_expires_at > CURRENT_TIMESTAMP",
                    &[tenant_id, request_id, token],
                )
                .await
                .map_err(db_error)?;
            transaction
                .execute(
                    "UPDATE checkout_payment_reconciliations SET updated_at=CURRENT_TIMESTAMP, lease_expires_at=CURRENT_TIMESTAMP + INTERVAL '5 minutes' WHERE tenant_id=$1 AND request_id=$2 AND checkout_lease_token=$3 AND status='processing' AND lease_expires_at > CURRENT_TIMESTAMP",
                    &[tenant_id, request_id, token],
                )
                .await
                .map_err(db_error)?;
            transaction.commit().await.map_err(db_error)
        }
    }
}

/// Lock the durable parent generation before changing any child or order
/// state. Reclaim takes this same lock, so a stale generation cannot pass a
/// check and then win a later state transition.
async fn lock_checkout_parent<C>(
    db: &C,
    tenant_id: &str,
    request_id: &str,
    parent_token: &str,
) -> ApiResult<bool>
where
    C: GenericClient + Sync,
{
    db.query_opt(
        "SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$2 AND a.lease_token=$3 AND (a.status='failed' OR (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP) OR (a.status='pending' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=a.tenant_id AND r.request_id=a.request_id AND r.checkout_lease_token=a.lease_token AND r.status='processing' AND r.lease_expires_at > CURRENT_TIMESTAMP))) FOR UPDATE",
        &[&tenant_id, &request_id, &parent_token],
    )
    .await
    .map(|row| row.is_some())
    .map_err(db_error)
}

pub(crate) async fn lock_checkout_fence<C>(db: &C, fence: &CheckoutFence) -> ApiResult<()>
where
    C: GenericClient + Sync,
{
    match fence {
        CheckoutFence::Attempt(lease) => {
            let current = db
                .query_opt(
                    "SELECT 1 FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$3 AND status='started' AND lease_expires_at > CURRENT_TIMESTAMP FOR UPDATE",
                    &[&lease.tenant_id, &lease.request_id, &lease.token],
                )
                .await
                .map_err(db_error)?
                .is_some();
            if current {
                Ok(())
            } else {
                Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    "checkout parent generation is no longer current",
                ))
            }
        }
        CheckoutFence::PaymentReconciliation {
            tenant_id,
            request_id,
            parent_token,
            worker_token,
        } => {
            let parent_current = db
                .query_opt(
                    "SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=$1 AND a.request_id=$2 AND a.lease_token=$3 AND (a.status='failed' OR (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP) OR (a.status='pending' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=a.tenant_id AND r.request_id=a.request_id AND r.checkout_lease_token=a.lease_token AND r.lease_token=$4 AND r.status='processing' AND r.lease_expires_at > CURRENT_TIMESTAMP))) FOR UPDATE",
                    &[tenant_id, request_id, parent_token, worker_token],
                )
                .await
                .map_err(db_error)?
                .is_some();
            if !parent_current {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    "checkout parent generation is no longer current",
                ));
            }
            let worker_current = db
                .query_opt(
                    "SELECT 1 FROM checkout_payment_reconciliations WHERE tenant_id=$1 AND request_id=$2 AND checkout_lease_token=$3 AND lease_token=$4 AND status='processing' AND lease_expires_at > CURRENT_TIMESTAMP FOR UPDATE",
                    &[tenant_id, request_id, parent_token, worker_token],
                )
                .await
                .map_err(db_error)?
                .is_some();
            if worker_current {
                Ok(())
            } else {
                Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    "payment reconciliation worker claim is no longer current",
                ))
            }
        }
        CheckoutFence::Compensation {
            tenant_id,
            request_id,
            token,
            task_id,
            claim_token,
        } => {
            if !lock_checkout_parent(db, tenant_id, request_id, token).await? {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    "checkout parent generation is no longer current",
                ));
            }
            let task_current = db
                .query_opt(
                    "SELECT 1 FROM checkout_compensation_tasks WHERE tenant_id=$1 AND id=$2 AND claim_token=$3 AND status='processing' AND claim_expires_at > CURRENT_TIMESTAMP AND checkout_request_id=$4 AND checkout_lease_token=$5 FOR UPDATE",
                    &[tenant_id, task_id, claim_token, request_id, token],
                )
                .await
                .map_err(db_error)?
                .is_some();
            if task_current {
                Ok(())
            } else {
                Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "checkout_lease_lost",
                    "compensation worker claim is no longer current",
                ))
            }
        }
    }
}

pub(crate) async fn assert_checkout_fence(
    state: &AppState,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let client = acquire_db(&state.pool).await?;
    let owned = match fence {
        CheckoutFence::Attempt(lease) => client
            .query_opt(
                "SELECT 1 FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$3 AND status='started' AND lease_expires_at > CURRENT_TIMESTAMP",
                &[&lease.tenant_id, &lease.request_id, &lease.token],
            )
            .await
            .map_err(db_error)?
            .is_some(),
        CheckoutFence::PaymentReconciliation {
            tenant_id,
            request_id,
            parent_token,
            worker_token,
        } => client
            .query_opt(
                "SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=$1 AND r.request_id=$2 AND r.checkout_lease_token=$3 AND r.lease_token=$4 AND r.status='processing' AND r.lease_expires_at > CURRENT_TIMESTAMP AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=r.tenant_id AND a.request_id=r.request_id AND a.lease_token=r.checkout_lease_token AND (a.status='failed' OR (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP) OR a.status='pending'))",
                &[tenant_id, request_id, parent_token, worker_token],
            )
            .await
            .map_err(db_error)?
            .is_some(),
        CheckoutFence::Compensation {
            tenant_id,
            request_id,
            token,
            task_id,
            claim_token,
        } => client
            .query_opt(
                "SELECT 1 FROM checkout_compensation_tasks t WHERE t.tenant_id=$1 AND t.id=$2 AND t.claim_token=$3 AND t.status='processing' AND t.claim_expires_at > CURRENT_TIMESTAMP AND t.checkout_request_id=$4 AND t.checkout_lease_token=$5 AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=t.tenant_id AND a.request_id=t.checkout_request_id AND a.lease_token=t.checkout_lease_token AND (a.status='failed' OR (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP) OR (a.status='pending' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=a.tenant_id AND r.request_id=a.request_id AND r.checkout_lease_token=a.lease_token AND r.status='processing' AND r.lease_expires_at > CURRENT_TIMESTAMP))))",
                &[tenant_id, task_id, claim_token, request_id, token],
            )
            .await
            .map_err(db_error)?
            .is_some(),
    };
    if owned {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            format!("{} lease is no longer current", fence.kind()),
        ))
    }
}

/// Record the remote compensation as in flight before crossing the service
/// boundary.  If the worker lease expires during the call, the marker remains
/// attached to the durable task and the successor retries the same idempotent
/// operation instead of losing the recovery obligation.
pub(crate) async fn begin_compensation_operation(
    state: &AppState,
    order: &PendingOrder,
    task_key: &str,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let operation_id = compensation_operation_id(task_key);
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let changed = transaction
        .execute(
            "UPDATE checkout_compensation_tasks SET remote_operation_id=$6, remote_operation_started_at=COALESCE(remote_operation_started_at,CURRENT_TIMESTAMP), remote_operation_completed_at=NULL, last_error=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND order_id=$2 AND task_key=$3 AND checkout_request_id=$4 AND checkout_lease_token=$5 AND status IN ('prepared','queued','processing')",
            &[
                &order.tenant_id,
                &order.id,
                &task_key,
                &fence.request_id(),
                &fence.token(),
                &operation_id,
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "compensation remote operation was not fenced by its durable task",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn complete_compensation_operation(
    state: &AppState,
    order: &PendingOrder,
    task_key: &str,
    fence: &CheckoutFence,
) -> ApiResult<()> {
    let operation_id = compensation_operation_id(task_key);
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    lock_checkout_fence(&transaction, fence).await?;
    let changed = transaction
        .execute(
            "UPDATE checkout_compensation_tasks SET remote_operation_completed_at=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND order_id=$2 AND task_key=$3 AND checkout_request_id=$4 AND checkout_lease_token=$5 AND remote_operation_id=$6 AND remote_operation_started_at IS NOT NULL AND status IN ('prepared','queued','processing')",
            &[
                &order.tenant_id,
                &order.id,
                &task_key,
                &fence.request_id(),
                &fence.token(),
                &operation_id,
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "compensation remote operation completion lost its durable fence",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) fn db_error(error: impl std::fmt::Display) -> ApiError {
    db_error_at("database", error)
}

pub(crate) fn db_error_at(stage: &str, error: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "database_unavailable",
        format!("{stage}: {error}"),
    )
}

pub(crate) async fn pricing_channel(state: &AppState) -> ApiResult<Channel> {
    shared_grpc_channel(
        &state.pricing_channel,
        &state.pricing_endpoint,
        "pricing_unavailable",
    )
    .await
}

pub(crate) async fn payment_channel(state: &AppState) -> ApiResult<Channel> {
    shared_grpc_channel(
        &state.payment_channel,
        &state.payment_endpoint,
        "payment_provider_unavailable",
    )
    .await
}

async fn shared_grpc_channel(
    cell: &Arc<OnceCell<Channel>>,
    endpoint: &str,
    error_code: &'static str,
) -> ApiResult<Channel> {
    cell.get_or_try_init(|| async {
        let endpoint = Endpoint::from_shared(endpoint.to_owned()).map_err(|error| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                error_code,
                format!("invalid gRPC endpoint: {error}"),
            )
        })?;
        endpoint
            .connect_timeout(Duration::from_secs(2))
            .connect()
            .await
            .map_err(|error| {
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    error_code,
                    error.to_string(),
                )
            })
    })
    .await
    .cloned()
}

pub(crate) fn postgres_pool(url: &str) -> anyhow::Result<Pool> {
    let mut config = Config::new();
    config.url = Some(url.to_owned());
    config.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    config.pool = Some(PoolConfig {
        max_size: 8,
        ..Default::default()
    });
    config
        .create_pool(Some(Runtime::Tokio1), tokio_postgres::NoTls)
        .context("create checkout postgres pool")
}

const RABBIT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RABBIT_RECONNECT_MIN_DELAY: Duration = Duration::from_secs(1);
const RABBIT_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);
const OUTBOX_PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);
const OUTBOX_LEASE_SECONDS: f64 = 30.0;

pub(crate) async fn init_rabbit() -> anyhow::Result<Arc<RabbitPublisher>> {
    let url = std::env::var("RABBITMQ_URL").unwrap_or_else(|_| DEFAULT_RABBITMQ_URL.to_owned());
    Ok(Arc::new(RabbitPublisher {
        url,
        active: Arc::new(Mutex::new(None)),
        next_generation: Arc::new(AtomicU64::new(1)),
    }))
}

impl RabbitPublisher {
    async fn ensure_channel(&self) -> anyhow::Result<RabbitChannel> {
        let mut active = self.active.lock().await;
        if let Some(connection) = active.as_ref()
            && connection.channel.status().connected()
        {
            return Ok(RabbitChannel {
                channel: connection.channel.clone(),
                generation: connection.generation,
            });
        }

        *active = None;
        let (connection, channel) =
            tokio::time::timeout(RABBIT_CONNECT_TIMEOUT, connect_rabbit(&self.url))
                .await
                .context("checkout RabbitMQ connection/topology setup timed out")??;
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        *active = Some(ActiveRabbit {
            generation,
            channel: channel.clone(),
            _connection: connection,
        });
        tracing::info!(generation, "checkout RabbitMQ publisher connected");
        Ok(RabbitChannel {
            channel,
            generation,
        })
    }

    async fn invalidate(&self, generation: u64) {
        let mut active = self.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|connection| connection.generation == generation)
        {
            *active = None;
            tracing::warn!(generation, "checkout RabbitMQ publisher connection lost");
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        let Ok(active) = self.active.try_lock() else {
            return false;
        };
        active
            .as_ref()
            .is_some_and(|connection| connection.channel.status().connected())
    }
}

async fn connect_rabbit(url: &str) -> anyhow::Result<(Arc<Connection>, lapin::Channel)> {
    let connection = Arc::new(
        Connection::connect(url, ConnectionProperties::default())
            .await
            .context("connect checkout RabbitMQ")?,
    );
    let channel = connection
        .create_channel()
        .await
        .context("create checkout RabbitMQ channel")?;
    declare_rabbit_topology(&channel).await?;
    channel
        .confirm_select(ConfirmSelectOptions::default())
        .await
        .context("enable checkout RabbitMQ publisher confirms")?;
    Ok((connection, channel))
}

async fn declare_rabbit_topology(channel: &lapin::Channel) -> anyhow::Result<()> {
    channel
        .exchange_declare(
            EXCHANGE,
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
        .context("declare commerce exchange")?;
    channel
        .exchange_declare(
            "commerce.dlx",
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
        .context("declare commerce dead-letter exchange")?;

    let orders_queue = configured_queue("FULFILLMENT_QUEUE", "fulfillment.orders");
    let analytics_queue = configured_queue("ANALYTICS_QUEUE", "fulfillment.analytics");
    declare_fulfillment_route(channel, &orders_queue).await?;
    declare_fulfillment_route(channel, &analytics_queue).await?;
    channel
        .queue_bind(
            &orders_queue,
            EXCHANGE,
            "order.#",
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await
        .context("bind checkout order outbox route")?;
    channel
        .queue_bind(
            &orders_queue,
            EXCHANGE,
            "payment.#",
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await
        .context("bind checkout payment outbox route")?;
    channel
        .queue_bind(
            &analytics_queue,
            EXCHANGE,
            "#",
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await
        .context("bind checkout analytics outbox route")?;
    Ok(())
}

async fn declare_fulfillment_route(channel: &lapin::Channel, queue: &str) -> anyhow::Result<()> {
    channel
        .queue_declare(
            queue,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            dead_letter_queue_arguments(queue),
        )
        .await
        .with_context(|| format!("declare fulfillment queue {queue}"))?;
    channel
        .queue_declare(
            &format!("{queue}.dead"),
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await
        .with_context(|| format!("declare fulfillment dead-letter queue {queue}.dead"))?;
    channel
        .queue_bind(
            &format!("{queue}.dead"),
            "commerce.dlx",
            &format!("{queue}.dead"),
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await
        .with_context(|| format!("bind fulfillment dead-letter queue {queue}.dead"))?;
    Ok(())
}

fn configured_queue(variable: &str, default: &str) -> String {
    std::env::var(variable)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn dead_letter_queue_arguments(queue: &str) -> FieldTable {
    let mut arguments = FieldTable::default();
    arguments.insert(
        "x-dead-letter-exchange".into(),
        AMQPValue::LongString("commerce.dlx".into()),
    );
    arguments.insert(
        "x-dead-letter-routing-key".into(),
        AMQPValue::LongString(format!("{queue}.dead").into()),
    );
    arguments
}

pub(crate) fn spawn_rabbit_supervisor(
    rabbit: Arc<RabbitPublisher>,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut delay = RABBIT_RECONNECT_MIN_DELAY;
        loop {
            if *shutdown.borrow() {
                break;
            }
            let result = tokio::select! {
                result = rabbit.ensure_channel() => Some(result),
                changed = shutdown.changed() => {
                    let _ = changed;
                    None
                }
            };
            let Some(result) = result else {
                break;
            };
            match result {
                Ok(_) => {
                    delay = RABBIT_RECONNECT_MIN_DELAY;
                    if wait_for_worker_or_shutdown(&mut shutdown, Duration::from_secs(1)).await {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        retry_after_ms = delay.as_millis(),
                        "checkout RabbitMQ reconnect failed"
                    );
                    if wait_for_worker_or_shutdown(&mut shutdown, delay).await {
                        break;
                    }
                    delay = (delay + delay).min(RABBIT_RECONNECT_MAX_DELAY);
                }
            }
        }
    })
}

async fn wait_for_worker_or_shutdown(
    shutdown: &mut watch::Receiver<bool>,
    delay: Duration,
) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        _ = tokio::time::sleep(delay) => false,
    }
}

pub(crate) async fn init_state(
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<(AppState, WorkerHandles)> {
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
    let pool = postgres_pool(&database_url)?;
    let client = tokio::time::timeout(DB_TIMEOUT, pool.get())
        .await
        .context("checkout postgres timeout")??;
    client
        .query_one("SELECT count(*) FROM orders", &[])
        .await
        .context("checkout schema missing; run mise run infra:postgres_migrate")?;
    drop(client);
    let rabbit = init_rabbit().await?;
    let state = AppState {
        pool,
        http: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()?,
        catalog_url: std::env::var("CATALOG_GRAPHQL_URL")
            .unwrap_or_else(|_| DEFAULT_CATALOG_URL.to_owned()),
        pricing_endpoint: std::env::var("PRICING_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_PRICING_ENDPOINT.to_owned()),
        pricing_channel: Arc::new(OnceCell::new()),
        payment_endpoint: std::env::var("PAYMENT_ENDPOINT")
            .unwrap_or_else(|_| DEFAULT_PAYMENT_ENDPOINT.to_owned()),
        payment_channel: Arc::new(OnceCell::new()),
        inventory_url: std::env::var("INVENTORY_URL")
            .unwrap_or_else(|_| DEFAULT_INVENTORY_URL.to_owned()),
        recommendation_url: std::env::var("RECOMMENDATION_URL")
            .unwrap_or_else(|_| DEFAULT_RECOMMENDATION_URL.to_owned()),
        rabbit: rabbit.clone(),
    };
    let workers = WorkerHandles {
        workers: vec![
            (
                "rabbit supervisor",
                spawn_rabbit_supervisor(rabbit, shutdown.clone()),
            ),
            (
                "outbox publisher",
                spawn_outbox_publisher(state.clone(), shutdown.clone()),
            ),
            (
                "compensation worker",
                spawn_compensation_worker(state.clone(), shutdown.clone()),
            ),
            (
                "payment reconciliation worker",
                spawn_payment_reconciliation_worker(state.clone(), shutdown),
            ),
        ],
    };
    Ok((state, workers))
}

pub(crate) fn spawn_payment_reconciliation_worker(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            match process_one_payment_reconciliation(&state).await {
                Ok(true) => continue,
                Ok(false) => {
                    if wait_for_worker_or_shutdown(
                        &mut shutdown,
                        PAYMENT_RECONCILIATION_POLL_INTERVAL,
                    )
                    .await
                    {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = %error.message,
                        "checkout payment reconciliation worker failed"
                    );
                    if wait_for_worker_or_shutdown(
                        &mut shutdown,
                        PAYMENT_RECONCILIATION_POLL_INTERVAL,
                    )
                    .await
                    {
                        break;
                    }
                }
            }
        }
    })
}

pub(crate) async fn process_one_payment_reconciliation(state: &AppState) -> ApiResult<bool> {
    if reclaim_exhausted_payment_reconciliation(state).await? {
        return Ok(true);
    }
    let Some(job) = claim_payment_reconciliation(state).await? else {
        return Ok(false);
    };
    let span = tracing::info_span!(
        "checkout.payment_reconciliation",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        "commerce.tenant_id" = %job.tenant_id,
        "commerce.order_id" = %job.order_id,
        "checkout.request_id" = %job.request_id,
        "payment.reconciliation_attempt" = job.attempts,
    );
    let parent = stored_reconciliation_context(&job)?;
    if parent.span().span_context().is_valid() {
        let _ = span.set_parent(parent);
    }
    let result = reconcile_pending_payment(state, &job)
        .instrument(span)
        .await;
    match result {
        Ok(PendingPaymentOutcome::Retry) => {
            reschedule_payment_reconciliation(state, &job, None).await?;
        }
        Ok(PendingPaymentOutcome::Completed(payload)) => {
            complete_payment_reconciliation(state, &job, &payload).await?;
        }
        Ok(PendingPaymentOutcome::Failed(error)) => {
            fail_payment_reconciliation(state, &job, &error).await?;
        }
        Err(error) if error.code == "checkout_lease_lost" => {
            tracing::info!(
                tenant_id = %job.tenant_id,
                request_id = %job.request_id,
                "payment reconciliation worker lost its lease"
            );
        }
        Err(error) => {
            reschedule_payment_reconciliation(state, &job, Some(&error)).await?;
        }
    }
    Ok(true)
}

pub(crate) async fn claim_payment_reconciliation(
    state: &AppState,
) -> ApiResult<Option<PaymentReconciliationJob>> {
    let lease_token = uuid::Uuid::new_v4().to_string();
    let client = acquire_db(&state.pool).await?;
    let row = client
        .query_opt(
            "WITH next_job AS (SELECT r.tenant_id, r.request_id, r.checkout_lease_token FROM checkout_payment_reconciliations r JOIN checkout_attempts a ON a.tenant_id=r.tenant_id AND a.request_id=r.request_id AND a.lease_token=r.checkout_lease_token WHERE ((r.status='queued' AND r.available_at <= CURRENT_TIMESTAMP AND r.attempts < $2) OR (r.status='processing' AND r.attempts < $2 AND r.lease_expires_at <= CURRENT_TIMESTAMP)) AND (a.status='failed' OR (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP) OR a.status='pending') ORDER BY r.available_at, r.created_at FOR UPDATE OF r, a SKIP LOCKED LIMIT 1) UPDATE checkout_payment_reconciliations job SET status='processing', attempts=job.attempts+1, claimed_at=CURRENT_TIMESTAMP, lease_token=$3, lease_expires_at=CURRENT_TIMESTAMP + ($1::double precision * INTERVAL '1 second'), updated_at=CURRENT_TIMESTAMP, last_error=NULL FROM next_job WHERE job.tenant_id=next_job.tenant_id AND job.request_id=next_job.request_id AND job.checkout_lease_token=next_job.checkout_lease_token RETURNING job.tenant_id, job.request_id, job.order_id, job.authorize_request_id, job.payment_id, job.merchant_reference, job.amount_minor, job.currency, job.method_type, job.feature_variant, job.status, job.attempts, job.lease_token, job.checkout_lease_token, job.traceparent, job.tracestate, job.baggage",
            &[
                &(PAYMENT_RECONCILIATION_STALE_AFTER.as_secs() as f64),
                &PAYMENT_RECONCILIATION_MAX_ATTEMPTS,
                &lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(PaymentReconciliationJob {
        tenant_id: row.get(0),
        request_id: row.get(1),
        order_id: row.get(2),
        authorize_request_id: row.get(3),
        payment_id: row.get(4),
        merchant_reference: row.get(5),
        amount_minor: row.get(6),
        currency: row.get(7),
        method_type: row.get(8),
        feature_variant: row.get(9),
        status: row.get(10),
        attempts: row.get(11),
        lease_token: row.get(12),
        parent_lease_token: row.get(13),
        traceparent: row.get(14),
        tracestate: row.get(15),
        baggage: row.get(16),
    }))
}

async fn reclaim_exhausted_payment_reconciliation(state: &AppState) -> ApiResult<bool> {
    let client = acquire_db(&state.pool).await?;
    let changed = client
        .execute(
            "UPDATE checkout_payment_reconciliations SET status='awaiting_provider', claimed_at=NULL, lease_token=NULL, lease_expires_at=NULL, available_at=CURRENT_TIMESTAMP, last_error=COALESCE(last_error, 'bounded checkout polling exhausted; waiting for payment provider reconciliation'), updated_at=CURRENT_TIMESTAMP WHERE status='processing' AND attempts >= $1 AND lease_expires_at <= CURRENT_TIMESTAMP",
            &[&PAYMENT_RECONCILIATION_MAX_ATTEMPTS],
        )
        .await
        .map_err(db_error)?;
    Ok(changed > 0)
}

pub(crate) async fn wake_payment_reconciliation(
    state: &AppState,
    tenant_id: &str,
    request_id: &str,
) -> ApiResult<()> {
    let client = acquire_db(&state.pool).await?;
    let status = client
        .query_opt(
            "SELECT status FROM checkout_payment_reconciliations WHERE tenant_id=$1 AND request_id=$2",
            &[&tenant_id, &request_id],
        )
        .await
        .map_err(db_error)?
        .map(|row| row.get::<_, String>(0))
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "payment_reconciliation_missing",
                "pending checkout has no durable payment reconciliation job",
            )
        })?;
    if matches!(status.as_str(), "awaiting_provider" | "queued") {
        client
            .execute(
                "UPDATE checkout_payment_reconciliations SET status='queued', available_at=CURRENT_TIMESTAMP, claimed_at=NULL, lease_token=NULL, lease_expires_at=NULL, last_error=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND status IN ('awaiting_provider','queued')",
                &[&tenant_id, &request_id],
            )
            .await
            .map_err(db_error)?;
    }
    Ok(())
}

pub(crate) async fn record_payment_reconciliation_observation(
    state: &AppState,
    job: &PaymentReconciliationJob,
    payment_status: &str,
    operation_status: &str,
    failure_reason: &str,
) -> ApiResult<()> {
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let fence = CheckoutFence::PaymentReconciliation {
        tenant_id: job.tenant_id.clone(),
        request_id: job.request_id.clone(),
        parent_token: job.parent_lease_token.clone(),
        worker_token: job.lease_token.clone(),
    };
    lock_checkout_fence(&transaction, &fence).await?;
    let changed = transaction
        .execute(
            "UPDATE checkout_payment_reconciliations SET last_payment_status=$3, last_operation_status=$4, last_failure_reason=$5, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND status='processing' AND checkout_lease_token=$6 AND lease_token=$7",
            &[
                &job.tenant_id,
                &job.request_id,
                &payment_status,
                &operation_status,
                &failure_reason,
                &job.parent_lease_token,
                &job.lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "payment reconciliation observation lease is no longer current",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn attach_payment_to_reconciliation(
    state: &AppState,
    job: &PaymentReconciliationJob,
    payment_id: &str,
) -> ApiResult<()> {
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let fence = CheckoutFence::PaymentReconciliation {
        tenant_id: job.tenant_id.clone(),
        request_id: job.request_id.clone(),
        parent_token: job.parent_lease_token.clone(),
        worker_token: job.lease_token.clone(),
    };
    lock_checkout_fence(&transaction, &fence).await?;
    let changed = transaction
        .execute(
            "UPDATE checkout_payment_reconciliations SET payment_id=COALESCE(payment_id,$3), updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND status='processing' AND checkout_lease_token=$4 AND lease_token=$5 AND (payment_id IS NULL OR payment_id=$3)",
            &[
                &job.tenant_id,
                &job.request_id,
                &payment_id,
                &job.parent_lease_token,
                &job.lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "payment_reconciliation_conflict",
            "payment identity does not match the reconciliation job",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn reschedule_payment_reconciliation(
    state: &AppState,
    job: &PaymentReconciliationJob,
    error: Option<&ApiError>,
) -> ApiResult<()> {
    let exhausted = job.attempts >= PAYMENT_RECONCILIATION_MAX_ATTEMPTS;
    let status = if exhausted {
        "awaiting_provider"
    } else {
        "queued"
    };
    let delay_seconds = PAYMENT_RECONCILIATION_RETRY_BASE
        .saturating_pow(job.attempts.clamp(1, 8) as u32)
        .min(300) as f64;
    let message = error.map(|value| bounded_error_message(&value.message));
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let fence = CheckoutFence::PaymentReconciliation {
        tenant_id: job.tenant_id.clone(),
        request_id: job.request_id.clone(),
        parent_token: job.parent_lease_token.clone(),
        worker_token: job.lease_token.clone(),
    };
    lock_checkout_fence(&transaction, &fence).await?;
    let changed = transaction
        .execute(
            "UPDATE checkout_payment_reconciliations SET status=$3, available_at=CASE WHEN $3='queued' THEN CURRENT_TIMESTAMP + ($4::double precision * INTERVAL '1 second') ELSE CURRENT_TIMESTAMP END, claimed_at=NULL, lease_token=NULL, lease_expires_at=NULL, last_error=COALESCE($5,last_error), updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND status='processing' AND checkout_lease_token=$6 AND lease_token=$7",
            &[
                &job.tenant_id,
                &job.request_id,
                &status,
                &delay_seconds,
                &message,
                &job.parent_lease_token,
                &job.lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    if changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "payment reconciliation retry lease is no longer current",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn complete_payment_reconciliation(
    state: &AppState,
    job: &PaymentReconciliationJob,
    payload: &Value,
) -> ApiResult<()> {
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let fence = CheckoutFence::PaymentReconciliation {
        tenant_id: job.tenant_id.clone(),
        request_id: job.request_id.clone(),
        parent_token: job.parent_lease_token.clone(),
        worker_token: job.lease_token.clone(),
    };
    lock_checkout_fence(&transaction, &fence).await?;
    let attempt_changed = transaction
        .execute(
            "UPDATE checkout_attempts SET status='paid', order_id=$3, response_payload=$4::jsonb, error_status=NULL, error_code=NULL, error_message=NULL, lease_expires_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$5 AND status IN ('pending','failed','started') AND order_id=$3",
            &[
                &job.tenant_id,
                &job.request_id,
                &job.order_id,
                payload,
                &job.parent_lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    if attempt_changed != 1 {
        let status = transaction
            .query_opt(
                "SELECT status FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2",
                &[&job.tenant_id, &job.request_id],
            )
            .await
            .map_err(db_error)?
            .map(|row| row.get::<_, String>(0));
        if status.as_deref() != Some("paid") {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "payment_reconciliation_attempt_conflict",
                "pending checkout attempt could not be finalized",
            ));
        }
    }
    let job_changed = transaction
        .execute(
            "UPDATE checkout_payment_reconciliations SET status='completed', completed_at=CURRENT_TIMESTAMP, claimed_at=NULL, lease_token=NULL, lease_expires_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND status='processing' AND checkout_lease_token=$3 AND lease_token=$4",
            &[
                &job.tenant_id,
                &job.request_id,
                &job.parent_lease_token,
                &job.lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    if job_changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "payment reconciliation completion lease is no longer current",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) async fn fail_payment_reconciliation(
    state: &AppState,
    job: &PaymentReconciliationJob,
    error: &ApiError,
) -> ApiResult<()> {
    let error_status = error.status.as_u16() as i16;
    let error_code = error.code;
    let message = bounded_error_message(&error.message);
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let fence = CheckoutFence::PaymentReconciliation {
        tenant_id: job.tenant_id.clone(),
        request_id: job.request_id.clone(),
        parent_token: job.parent_lease_token.clone(),
        worker_token: job.lease_token.clone(),
    };
    lock_checkout_fence(&transaction, &fence).await?;
    let attempt_changed = transaction
        .execute(
            "UPDATE checkout_attempts SET status='failed', response_payload=NULL, error_status=$3, error_code=$4, error_message=$5, lease_expires_at=NULL, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND lease_token=$7 AND status IN ('pending','failed','started') AND order_id=$6",
            &[
                &job.tenant_id,
                &job.request_id,
                &error_status,
                &error_code,
                &message,
                &job.order_id,
                &job.parent_lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    if attempt_changed == 0 {
        let status = transaction
            .query_opt(
                "SELECT status FROM checkout_attempts WHERE tenant_id=$1 AND request_id=$2",
                &[&job.tenant_id, &job.request_id],
            )
            .await
            .map_err(db_error)?
            .map(|row| row.get::<_, String>(0));
        if status.as_deref() != Some("failed") {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "payment_reconciliation_attempt_conflict",
                "pending checkout attempt could not be failed",
            ));
        }
    }
    transaction
        .execute(
            "UPDATE orders o SET status='cancelled', updated_at=CURRENT_TIMESTAMP WHERE o.tenant_id=$1 AND o.id=$2 AND o.status='pending' AND NOT EXISTS (SELECT 1 FROM checkout_compensation_tasks t WHERE t.tenant_id=o.tenant_id AND t.order_id=o.id AND t.status NOT IN ('completed','superseded'))",
            &[&job.tenant_id, &job.order_id],
        )
        .await
        .map_err(db_error)?;
    let job_changed = transaction
        .execute(
            "UPDATE checkout_payment_reconciliations SET status='failed', completed_at=NULL, claimed_at=NULL, lease_token=NULL, lease_expires_at=NULL, last_error=$3, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND request_id=$2 AND status='processing' AND checkout_lease_token=$4 AND lease_token=$5",
            &[
                &job.tenant_id,
                &job.request_id,
                &message,
                &job.parent_lease_token,
                &job.lease_token,
            ],
        )
        .await
        .map_err(db_error)?;
    if job_changed != 1 {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "payment reconciliation failure lease is no longer current",
        ));
    }
    transaction.commit().await.map_err(db_error)
}

pub(crate) fn spawn_compensation_worker(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            match process_one_compensation(&state).await {
                Ok(true) => continue,
                Ok(false) => {
                    if wait_for_worker_or_shutdown(&mut shutdown, COMPENSATION_POLL_INTERVAL).await
                    {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(
                        error = %error.message,
                        "checkout compensation worker failed"
                    );
                    if wait_for_worker_or_shutdown(&mut shutdown, Duration::from_secs(1)).await {
                        break;
                    }
                }
            }
        }
    })
}

pub(crate) async fn claim_compensation_task(
    state: &AppState,
) -> ApiResult<Option<CompensationTask>> {
    let client = acquire_db(&state.pool).await?;
    let row = client
        .query_opt(
            "WITH next_task AS (SELECT t.tenant_id, t.id, t.checkout_request_id, t.checkout_lease_token FROM checkout_compensation_tasks t JOIN checkout_attempts a ON a.tenant_id=t.tenant_id AND a.request_id=t.checkout_request_id AND a.lease_token=t.checkout_lease_token WHERE t.checkout_request_id IS NOT NULL AND t.checkout_lease_token IS NOT NULL AND t.attempts < $2 AND ((t.status IN ('prepared','queued') AND t.available_at <= CURRENT_TIMESTAMP) OR (t.status='processing' AND t.claim_expires_at <= CURRENT_TIMESTAMP)) AND (a.status='failed' OR (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP) OR (a.status='pending' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=a.tenant_id AND r.request_id=a.request_id AND r.checkout_lease_token=a.lease_token AND r.status='processing' AND r.lease_expires_at > CURRENT_TIMESTAMP))) ORDER BY t.available_at, t.created_at FOR UPDATE OF t, a SKIP LOCKED LIMIT 1) UPDATE checkout_compensation_tasks task SET status='processing', attempts=task.attempts+1, claim_token=$3, claim_expires_at=CURRENT_TIMESTAMP + ($1::double precision * INTERVAL '1 second'), claimed_at=CURRENT_TIMESTAMP, updated_at=CURRENT_TIMESTAMP, last_error=NULL FROM next_task WHERE task.tenant_id=next_task.tenant_id AND task.id=next_task.id AND task.checkout_request_id=next_task.checkout_request_id AND task.checkout_lease_token=next_task.checkout_lease_token RETURNING task.id, task.tenant_id, task.order_id, task.kind, task.reservation_id, task.sku, task.quantity, task.payment_id, task.request_id, task.currency, task.attempts, task.traceparent, task.tracestate, task.baggage, task.checkout_request_id, task.checkout_lease_token, task.claim_token",
            &[
                &(COMPENSATION_STALE_AFTER.as_secs() as f64),
                &COMPENSATION_MAX_ATTEMPTS,
                &uuid::Uuid::new_v4().to_string(),
            ],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let quantity = row
        .get::<_, Option<i32>>(6)
        .map(|value| {
            u32::try_from(value).map_err(|_| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "compensation_persistence_failed",
                    "stored compensation quantity is out of range",
                )
            })
        })
        .transpose()?;
    Ok(Some(CompensationTask {
        id: row.get(0),
        tenant_id: row.get(1),
        order_id: row.get(2),
        kind: row.get(3),
        reservation_id: row.get(4),
        sku: row.get(5),
        quantity,
        payment_id: row.get(7),
        request_id: row.get(8),
        currency: row.get(9),
        attempts: row.get(10),
        traceparent: row.get(11),
        tracestate: row.get(12),
        baggage: row.get(13),
        checkout_request_id: row.get(14),
        checkout_lease_token: row.get(15),
        claim_token: row.get(16),
    }))
}

pub(crate) async fn process_one_compensation(state: &AppState) -> ApiResult<bool> {
    if dead_letter_expired_compensation_task(state).await? {
        return Ok(true);
    }
    let Some(task) = claim_compensation_task(state).await? else {
        return Ok(false);
    };
    let span = tracing::info_span!(
        "checkout.compensation",
        otel.kind = semconv::SPAN_KIND_INTERNAL,
        "compensation.task_id" = %task.id,
        "compensation.kind" = %task.kind,
        "commerce.tenant_id" = %task.tenant_id,
        "commerce.order_id" = %task.order_id,
    );
    let parent = stored_compensation_context(&task)?;
    if parent.span().span_context().is_valid() {
        let _ = span.set_parent(parent.clone());
    }
    playground_telemetry::stamp_business_baggage(&span, &parent);
    let result = process_compensation_task(state, &task)
        .instrument(span)
        .await;
    match result {
        Ok(()) => complete_compensation_task(state, &task).await?,
        Err(error) => {
            if compensation_attempts_exhausted(task.attempts) {
                tracing::error!(
                    task_id = %task.id,
                    attempts = task.attempts,
                    error = %error.message,
                    "durable checkout compensation dead-lettered after bounded retries"
                );
                dead_letter_compensation_task(state, &task, &error).await?;
            } else {
                tracing::warn!(
                    task_id = %task.id,
                    attempts = task.attempts,
                    error = %error.message,
                    "durable checkout compensation will retry"
                );
                reschedule_compensation_task(state, &task, &error).await?;
            }
        }
    }
    Ok(true)
}

pub(crate) async fn process_compensation_task(
    state: &AppState,
    task: &CompensationTask,
) -> ApiResult<()> {
    let parent = stored_compensation_context(task)?;
    let context = playground_telemetry::with_safe_parent_baggage(
        &tracing::Span::current().context(),
        &parent,
    );
    let timeout_ms = DB_TIMEOUT.as_millis() as u64;
    match task.kind.as_str() {
        "inventory_release" => {
            let fence = compensation_fence(state, task).await?;
            let order = load_pending_order(state, &task.tenant_id, &task.order_id).await?;
            let reservation_id = task.reservation_id.clone().ok_or_else(|| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "compensation_persistence_failed",
                    "inventory compensation task has no reservation_id",
                )
            })?;
            let task_key = generation_compensation_task_key(
                &inventory_compensation_task_key(&reservation_id),
                fence.token(),
            );
            begin_compensation_operation(state, &order, &task_key, &fence).await?;
            let reservation = InventoryReservation {
                reservation_id,
                sku: task.sku.clone().ok_or_else(|| {
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "compensation_persistence_failed",
                        "inventory compensation task has no SKU",
                    )
                })?,
                quantity: task.quantity.ok_or_else(|| {
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "compensation_persistence_failed",
                        "inventory compensation task has no quantity",
                    )
                })?,
                location_id: None,
            };
            let result =
                release_inventory(state, &context, &order, &[reservation], &fence, timeout_ms)
                    .await;
            if result.is_ok() {
                complete_compensation_operation(state, &order, &task_key, &fence).await?;
            }
            result
        }
        "payment" => {
            let fence = compensation_fence(state, task).await?;
            let order = load_pending_order(state, &task.tenant_id, &task.order_id).await?;
            if task.currency.as_deref() != Some(order.currency.as_str()) {
                return Err(ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "compensation_persistence_failed",
                    "payment compensation currency does not match the order",
                ));
            }
            let payment_id = task.payment_id.as_deref().ok_or_else(|| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "compensation_persistence_failed",
                    "payment compensation task has no payment_id",
                )
            })?;
            let request_id = task.request_id.as_deref().unwrap_or(task.id.as_str());
            if compensate_payment_with_fence(
                state, &context, &order, payment_id, request_id, timeout_ms, &fence,
            )
            .await?
            {
                Ok(())
            } else {
                Err(payment_reconciliation_error(
                    "durable payment compensation is not yet confirmed",
                ))
            }
        }
        _ => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "compensation_persistence_failed",
            format!("unknown compensation task kind: {}", task.kind),
        )),
    }
}

async fn compensation_fence(state: &AppState, task: &CompensationTask) -> ApiResult<CheckoutFence> {
    let request_id = task.checkout_request_id.as_deref().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "compensation task has no checkout fence",
        )
    })?;
    let lease_token = task.checkout_lease_token.as_deref().ok_or_else(|| {
        ApiError::new(
            StatusCode::CONFLICT,
            "checkout_lease_lost",
            "compensation task has no checkout lease token",
        )
    })?;
    let client = acquire_db(&state.pool).await?;
    let current = client
        .query_opt(
            "SELECT 1 FROM checkout_compensation_tasks t WHERE t.tenant_id=$1 AND t.id=$2 AND t.claim_token=$3 AND t.status='processing' AND t.claim_expires_at > CURRENT_TIMESTAMP AND t.checkout_request_id=$4 AND t.checkout_lease_token=$5 AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=t.tenant_id AND a.request_id=t.checkout_request_id AND a.lease_token=t.checkout_lease_token AND (a.status='failed' OR (a.status='started' AND a.lease_expires_at > CURRENT_TIMESTAMP) OR (a.status='pending' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=a.tenant_id AND r.request_id=a.request_id AND r.checkout_lease_token=a.lease_token AND r.status='processing' AND r.lease_expires_at > CURRENT_TIMESTAMP))))",
            &[&task.tenant_id, &task.id, &task.claim_token, &request_id, &lease_token],
        )
        .await
        .map_err(db_error)?
        .is_some();
    if current {
        return Ok(CheckoutFence::Compensation {
            tenant_id: task.tenant_id.clone(),
            request_id: request_id.to_owned(),
            token: lease_token.to_owned(),
            task_id: task.id.clone(),
            claim_token: task.claim_token.clone(),
        });
    }
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "checkout_lease_lost",
        "compensation task fence is no longer current",
    ))
}

pub(crate) async fn complete_compensation_task(
    state: &AppState,
    task: &CompensationTask,
) -> ApiResult<()> {
    finish_compensation_task(state, task, None).await
}

pub(crate) async fn dead_letter_compensation_task(
    state: &AppState,
    task: &CompensationTask,
    error: &ApiError,
) -> ApiResult<()> {
    let message = format!(
        "dead-lettered after {} attempts: {}",
        task.attempts,
        bounded_error_message(&error.message)
    );
    finish_compensation_task(state, task, Some(&message)).await
}

pub(crate) async fn finish_compensation_task(
    state: &AppState,
    task: &CompensationTask,
    terminal_error: Option<&str>,
) -> ApiResult<()> {
    let terminal_error = terminal_error.map(str::to_owned);
    let terminal_status = if terminal_error.is_some() {
        "failed"
    } else {
        "completed"
    };
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let fence = CheckoutFence::Compensation {
        tenant_id: task.tenant_id.clone(),
        request_id: task.checkout_request_id.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "checkout_lease_lost",
                "compensation task has no checkout fence",
            )
        })?,
        token: task.checkout_lease_token.clone().ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "checkout_lease_lost",
                "compensation task has no checkout lease token",
            )
        })?,
        task_id: task.id.clone(),
        claim_token: task.claim_token.clone(),
    };
    lock_checkout_fence(&transaction, &fence).await?;
    let completed = transaction
        .execute(
            "UPDATE checkout_compensation_tasks SET status=$3, completed_at=CASE WHEN $3='completed' THEN CURRENT_TIMESTAMP ELSE NULL END, claimed_at=NULL, claim_token=NULL, claim_expires_at=NULL, last_error=$4, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$1 AND id=$2 AND status='processing' AND claim_token=$5 AND claim_expires_at > CURRENT_TIMESTAMP",
            &[&task.tenant_id, &task.id, &terminal_status, &terminal_error, &task.claim_token],
        )
        .await
        .map_err(db_error)?;
    if completed != 1 {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "compensation_persistence_failed",
            "compensation task ownership was lost before completion",
        ));
    }
    if terminal_error.is_none() {
        transaction
            .execute(
                "UPDATE orders o SET status='cancelled', updated_at=CURRENT_TIMESTAMP WHERE o.tenant_id=$1 AND o.id=$2 AND o.status='pending' AND NOT EXISTS (SELECT 1 FROM checkout_compensation_tasks t WHERE t.tenant_id=o.tenant_id AND t.order_id=o.id AND t.status NOT IN ('completed','superseded')) AND EXISTS (SELECT 1 FROM checkout_compensation_tasks t WHERE t.id=$3 AND t.tenant_id=o.tenant_id AND t.order_id=o.id AND t.checkout_request_id IS NOT NULL AND t.checkout_lease_token IS NOT NULL AND EXISTS (SELECT 1 FROM checkout_attempts a WHERE a.tenant_id=t.tenant_id AND a.request_id=t.checkout_request_id AND a.lease_token=t.checkout_lease_token AND (a.status IN ('started','failed') OR (a.status='pending' AND EXISTS (SELECT 1 FROM checkout_payment_reconciliations r WHERE r.tenant_id=a.tenant_id AND r.request_id=a.request_id AND r.checkout_lease_token=a.lease_token AND r.status='processing')))))",
                &[&task.tenant_id, &task.order_id, &task.id],
            )
            .await
            .map_err(db_error)?;
    }
    transaction.commit().await.map_err(db_error)?;
    Ok(())
}

pub(crate) async fn dead_letter_expired_compensation_task(state: &AppState) -> ApiResult<bool> {
    let message = format!(
        "dead-lettered after {} attempts: worker lease expired",
        COMPENSATION_MAX_ATTEMPTS
    );
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    let row = transaction
        .query_opt(
            "WITH expired_task AS (SELECT tenant_id, id FROM checkout_compensation_tasks WHERE status='processing' AND attempts >= $1 AND claim_expires_at <= CURRENT_TIMESTAMP AND updated_at < CURRENT_TIMESTAMP - ($2::double precision * INTERVAL '1 second') ORDER BY updated_at FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE checkout_compensation_tasks task SET status='failed', completed_at=NULL, claimed_at=NULL, claim_token=NULL, claim_expires_at=NULL, last_error=COALESCE(task.last_error, $3), updated_at=CURRENT_TIMESTAMP FROM expired_task WHERE task.tenant_id=expired_task.tenant_id AND task.id=expired_task.id RETURNING task.id",
            &[
                &COMPENSATION_MAX_ATTEMPTS,
                &(COMPENSATION_STALE_AFTER.as_secs() as f64),
                &message,
            ],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        return Ok(false);
    };
    let task_id: String = row.get(0);
    transaction.commit().await.map_err(db_error)?;
    tracing::error!(
        task_id = %task_id,
        attempts = COMPENSATION_MAX_ATTEMPTS,
        "expired compensation lease moved to the durable dead-letter state"
    );
    Ok(true)
}

pub(crate) async fn reschedule_compensation_task(
    state: &AppState,
    task: &CompensationTask,
    error: &ApiError,
) -> ApiResult<()> {
    let delay_seconds = COMPENSATION_RETRY_BASE
        .saturating_pow(task.attempts.clamp(1, 8) as u32)
        .min(300) as f64;
    let message = bounded_error_message(&error.message);
    let client = acquire_db(&state.pool).await?;
    client
        .execute(
            "UPDATE checkout_compensation_tasks SET status='queued', available_at=CURRENT_TIMESTAMP + ($2::double precision * INTERVAL '1 second'), claimed_at=NULL, claim_token=NULL, claim_expires_at=NULL, last_error=$3, updated_at=CURRENT_TIMESTAMP WHERE tenant_id=$4 AND id=$1 AND status='processing' AND claim_token=$5 AND claim_expires_at > CURRENT_TIMESTAMP",
            &[&task.id, &delay_seconds, &message, &task.tenant_id, &task.claim_token],
        )
        .await
        .map_err(db_error)?;
    Ok(())
}

pub(crate) fn compensation_attempts_exhausted(attempts: i32) -> bool {
    attempts >= COMPENSATION_MAX_ATTEMPTS
}

pub(crate) fn bounded_error_message(message: &str) -> String {
    message.chars().take(COMPENSATION_ERROR_LIMIT).collect()
}

#[derive(Debug)]

pub(crate) struct OutboxRow {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) event_key: String,
    pub(crate) aggregate_type: String,
    pub(crate) aggregate_id: String,
    pub(crate) event_type: String,
    pub(crate) schema_version: i32,
    pub(crate) occurred_at: String,
    pub(crate) payload: String,
    pub(crate) traceparent: Option<String>,
    pub(crate) tracestate: Option<String>,
    pub(crate) baggage: Option<String>,
    pub(crate) attempts: i32,
    pub(crate) claim_token: String,
}

const OUTBOX_MAX_ATTEMPTS: i32 = 3;
const OUTBOX_RETRY_BASE_SECONDS: i32 = 2;
const OUTBOX_ERROR_LIMIT: usize = 1_000;

struct OutboxClaim {
    event: OutboxRow,
    client: deadpool_postgres::Client,
}

pub(crate) fn spawn_outbox_publisher(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            match publish_one_outbox(&state).await {
                Ok(true) => continue,
                Ok(false) => {
                    if wait_for_worker_or_shutdown(&mut shutdown, Duration::from_millis(250)).await
                    {
                        break;
                    }
                }
                Err(error) => {
                    tracing::error!(error = %error.message, "checkout outbox publisher failed");
                    if wait_for_worker_or_shutdown(&mut shutdown, Duration::from_secs(1)).await {
                        break;
                    }
                }
            }
        }
    })
}

async fn claim_outbox_event(state: &AppState) -> ApiResult<Option<OutboxClaim>> {
    let claim_token = uuid::Uuid::new_v4().to_string();
    let mut client = acquire_db(&state.pool).await?;
    let transaction = client.transaction().await.map_err(db_error)?;
    transaction
        .execute(
            "UPDATE outbox_events SET status='failed', failure_code=COALESCE(failure_code, 'publish_retry_exhausted'), failure_message=COALESCE(failure_message, 'outbox publish retry budget exhausted'), failed_at=COALESCE(failed_at, CURRENT_TIMESTAMP), claim_token=NULL, claim_expires_at=NULL WHERE attempts >= $1 AND (status='queued' OR (status='processing' AND claim_expires_at <= CURRENT_TIMESTAMP))",
            &[&OUTBOX_MAX_ATTEMPTS],
        )
        .await
        .map_err(db_error)?;
    let row = transaction
        .query_opt(
            "WITH next_event AS (SELECT id FROM outbox_events WHERE attempts < $1 AND ((status='queued' AND available_at <= CURRENT_TIMESTAMP) OR (status='processing' AND claim_expires_at <= CURRENT_TIMESTAMP)) ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1) UPDATE outbox_events AS event SET status='processing', attempts=event.attempts+1, available_at=CURRENT_TIMESTAMP + ($2::double precision * INTERVAL '1 second'), claim_token=$3, claim_expires_at=CURRENT_TIMESTAMP + ($2::double precision * INTERVAL '1 second') FROM next_event WHERE event.id=next_event.id RETURNING event.id,event.tenant_id,event.event_key,event.aggregate_type,event.aggregate_id,event.event_type,event.schema_version,to_char(event.occurred_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'),event.payload::text,event.traceparent,event.tracestate,event.baggage,event.attempts,event.claim_token",
            &[&OUTBOX_MAX_ATTEMPTS, &OUTBOX_LEASE_SECONDS, &claim_token],
        )
        .await
        .map_err(db_error)?;
    let Some(row) = row else {
        transaction.commit().await.map_err(db_error)?;
        return Ok(None);
    };
    let event = OutboxRow {
        id: row.get(0),
        tenant_id: row.get(1),
        event_key: row.get(2),
        aggregate_type: row.get(3),
        aggregate_id: row.get(4),
        event_type: row.get(5),
        schema_version: row.get(6),
        occurred_at: row.get(7),
        payload: row.get(8),
        traceparent: row.get(9),
        tracestate: row.get(10),
        baggage: row.get(11),
        attempts: row.get(12),
        claim_token: row.get(13),
    };
    if let Err(error) = transaction.commit().await {
        return Err(db_error_at("claim outbox event", error));
    }
    Ok(Some(OutboxClaim { event, client }))
}

pub(crate) async fn publish_one_outbox(state: &AppState) -> ApiResult<bool> {
    let Some(claim) = claim_outbox_event(state).await? else {
        return Ok(false);
    };
    let result = publish_outbox_message(state, &claim.event).await;
    match result {
        Ok(()) => {
            let update_result = claim
                .client
                .execute(
                    "UPDATE outbox_events SET status='published', published_at=CURRENT_TIMESTAMP, failure_code=NULL, failure_message=NULL, failed_at=NULL, claim_token=NULL, claim_expires_at=NULL WHERE id=$1 AND status='processing' AND claim_token=$2 AND claim_expires_at > CURRENT_TIMESTAMP",
                    &[&claim.event.id, &claim.event.claim_token],
                )
                .await
                .map_err(db_error);
            let updated = update_result?;
            if updated != 1 {
                return Err(ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "outbox_state_conflict",
                    "confirmed outbox event could not be marked published",
                ));
            }
            Ok(true)
        }
        Err(error) => {
            let next_status = outbox_failure_status(claim.event.attempts, error.code);
            let delay = outbox_retry_delay(claim.event.attempts) as f64;
            let failure_message = bounded_outbox_error(&error.message);
            let update_result = claim
                .client
                .execute(
                    "UPDATE outbox_events SET status=$2, available_at=CASE WHEN $2='queued' THEN CURRENT_TIMESTAMP + ($3::double precision * INTERVAL '1 second') ELSE CURRENT_TIMESTAMP END, failure_code=$4, failure_message=$5, failed_at=CASE WHEN $2='failed' THEN CURRENT_TIMESTAMP ELSE NULL END, claim_token=NULL, claim_expires_at=NULL WHERE id=$1 AND status='processing' AND claim_token=$6 AND claim_expires_at > CURRENT_TIMESTAMP",
                    &[
                        &claim.event.id,
                        &next_status,
                        &delay,
                        &error.code,
                        &failure_message,
                        &claim.event.claim_token,
                    ],
                )
                .await
                .map_err(db_error);
            let updated = update_result?;
            if updated != 1 {
                return Err(ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "outbox_state_conflict",
                    "failed outbox event could not be durably classified",
                ));
            }
            if next_status == "failed" {
                tracing::error!(
                    outbox_id = %claim.event.id,
                    event_key = %claim.event.event_key,
                    attempts = claim.event.attempts,
                    error_code = error.code,
                    "outbox event dead-lettered after bounded publish retries; operator replay required"
                );
            } else {
                tracing::warn!(
                    outbox_id = %claim.event.id,
                    event_key = %claim.event.event_key,
                    attempts = claim.event.attempts,
                    retry_after_seconds = delay,
                    error_code = error.code,
                    "outbox event publish failed; retry scheduled"
                );
            }
            Err(error)
        }
    }
}

fn outbox_failure_status(attempts: i32, error_code: &str) -> &'static str {
    if !is_transient_outbox_error(error_code) || attempts >= OUTBOX_MAX_ATTEMPTS {
        "failed"
    } else {
        "queued"
    }
}

fn is_transient_outbox_error(error_code: &str) -> bool {
    matches!(
        error_code,
        "messaging_unavailable" | "messaging_timeout" | "messaging_rejected"
    )
}

fn outbox_retry_delay(attempts: i32) -> i32 {
    attempts
        .saturating_mul(OUTBOX_RETRY_BASE_SECONDS)
        .clamp(1, 30)
}

fn bounded_outbox_error(message: &str) -> String {
    message.chars().take(OUTBOX_ERROR_LIMIT).collect()
}

fn require_routed_confirmation(confirmation: Confirmation) -> ApiResult<()> {
    match confirmation {
        Confirmation::Ack(None) => Ok(()),
        Confirmation::Ack(Some(_)) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "messaging_unroutable",
            "RabbitMQ returned the published event because no route accepted it",
        )),
        Confirmation::Nack(_) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "messaging_rejected",
            "RabbitMQ negatively acknowledged the published event",
        )),
        Confirmation::NotRequested => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "messaging_confirm_unavailable",
            "RabbitMQ publisher confirmation was not requested",
        )),
    }
}

pub(crate) async fn publish_outbox_message(state: &AppState, event: &OutboxRow) -> ApiResult<()> {
    let parent = stored_event_context(event)?;
    let span = tracing::info_span!(
        "checkout.outbox.publish",
        otel.kind = semconv::SPAN_KIND_PRODUCER,
        "messaging.system" = "rabbitmq",
        "messaging.destination.name" = EXCHANGE,
        "messaging.operation.name" = "send",
        "messaging.message.id" = %event.id,
        "messaging.delivery.attempt" = event.attempts,
        "commerce.tenant_id" = %event.tenant_id,
        "commerce.event.type" = %event.event_type,
    );
    if parent.span().span_context().is_valid() {
        let _ = span.set_parent(parent.clone());
    }
    playground_telemetry::stamp_business_baggage(&span, &parent);
    async move {
        let payload: Value = serde_json::from_str(&event.payload).map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "event_protocol_error",
                error.to_string(),
            )
        })?;
        let envelope = json!({
            "event_id": event.id,
            "event_key": event.event_key,
            "schema_version": event.schema_version,
            "tenant_id": event.tenant_id,
            "event_type": event.event_type,
            "occurred_at": event.occurred_at,
            "aggregate_type": event.aggregate_type,
            "aggregate_id": event.aggregate_id,
            "payload": payload
        });
        let context = playground_telemetry::with_safe_parent_baggage(
            &tracing::Span::current().context(),
            &parent,
        );
        let mut propagation_headers = HeaderMap::new();
        playground_telemetry::inject_context_headers(&context, &mut propagation_headers);
        let mut headers = FieldTable::default();
        for key in ["traceparent", "tracestate", "baggage"] {
            if let Some(value) = propagation_headers
                .get(key)
                .and_then(|value| value.to_str().ok())
            {
                insert_header(&mut headers, key, Some(value));
            }
        }
        insert_header(&mut headers, "event-key", Some(&event.event_key));
        insert_header(&mut headers, "event-type", Some(&event.event_type));
        insert_header(&mut headers, "tenant-id", Some(&event.tenant_id));
        let rabbit = state.rabbit.ensure_channel().await.map_err(|error| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "messaging_unavailable",
                error.to_string(),
            )
        })?;
        let body = serde_json::to_vec(&envelope).map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "event_encode_failed",
                error.to_string(),
            )
        })?;
        let properties = BasicProperties::default()
            .with_content_type("application/json".into())
            .with_delivery_mode(2)
            .with_message_id(event.id.clone().into())
            .with_headers(headers);
        let publication = async {
            rabbit
                .channel
                .basic_publish(
                    EXCHANGE,
                    &event.event_type,
                    BasicPublishOptions {
                        mandatory: true,
                        ..BasicPublishOptions::default()
                    },
                    &body,
                    properties,
                )
                .await
                .map_err(|error| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "messaging_unavailable",
                        error.to_string(),
                    )
                })?
                .await
                .map_err(|error| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "messaging_rejected",
                        error.to_string(),
                    )
                })
        };
        let confirmation = match tokio::time::timeout(OUTBOX_PUBLISH_TIMEOUT, publication).await {
            Ok(Ok(confirmation)) => confirmation,
            Ok(Err(error)) => {
                state.rabbit.invalidate(rabbit.generation).await;
                return Err(error);
            }
            Err(_) => {
                state.rabbit.invalidate(rabbit.generation).await;
                return Err(ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "messaging_timeout",
                    "RabbitMQ publish confirmation timed out",
                ));
            }
        };
        require_routed_confirmation(confirmation)?;
        Ok(())
    }
    .instrument(span)
    .await
}

fn stored_event_context(event: &OutboxRow) -> ApiResult<opentelemetry::Context> {
    stored_carrier_context(
        event.traceparent.as_deref(),
        event.tracestate.as_deref(),
        event.baggage.as_deref(),
    )
}

fn stored_compensation_context(task: &CompensationTask) -> ApiResult<opentelemetry::Context> {
    stored_carrier_context(
        task.traceparent.as_deref(),
        task.tracestate.as_deref(),
        task.baggage.as_deref(),
    )
}

pub(crate) fn stored_reconciliation_context(
    job: &PaymentReconciliationJob,
) -> ApiResult<opentelemetry::Context> {
    stored_carrier_context(
        job.traceparent.as_deref(),
        job.tracestate.as_deref(),
        job.baggage.as_deref(),
    )
}

fn stored_carrier_context(
    traceparent: Option<&str>,
    tracestate: Option<&str>,
    baggage: Option<&str>,
) -> ApiResult<opentelemetry::Context> {
    let mut headers = HeaderMap::new();
    insert_w3c_header(
        &mut headers,
        HeaderName::from_static("traceparent"),
        traceparent,
    )?;
    insert_w3c_header(
        &mut headers,
        HeaderName::from_static("tracestate"),
        tracestate,
    )?;
    insert_w3c_header(&mut headers, HeaderName::from_static("baggage"), baggage)?;
    playground_telemetry::extract_durable_context(&headers).map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "stored_propagation_invalid",
            error.to_string(),
        )
    })
}

fn insert_w3c_header(
    headers: &mut HeaderMap,
    key: HeaderName,
    value: Option<&str>,
) -> ApiResult<()> {
    let value = value.filter(|value| !value.is_empty()).ok_or_else(|| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "stored_propagation_invalid",
            format!("stored propagation field {} is missing", key.as_str()),
        )
    })?;
    let value = HeaderValue::from_str(value).map_err(|error| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "stored_propagation_invalid",
            format!(
                "stored propagation field {} is invalid: {error}",
                key.as_str()
            ),
        )
    })?;
    headers.insert(key, value);
    Ok(())
}

pub(crate) fn insert_header(headers: &mut FieldTable, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        headers.insert(key.into(), AMQPValue::LongString(value.to_owned().into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ack_without_a_returned_message_is_routed_success() {
        assert!(require_routed_confirmation(Confirmation::Ack(None)).is_ok());

        let negative = require_routed_confirmation(Confirmation::Nack(None)).unwrap_err();
        assert_eq!(negative.code, "messaging_rejected");

        let missing = require_routed_confirmation(Confirmation::NotRequested).unwrap_err();
        assert_eq!(missing.code, "messaging_confirm_unavailable");
    }

    #[test]
    fn outbox_retry_delay_is_bounded() {
        assert_eq!(outbox_retry_delay(0), 1);
        assert_eq!(outbox_retry_delay(1), OUTBOX_RETRY_BASE_SECONDS);
        assert_eq!(outbox_retry_delay(2), OUTBOX_RETRY_BASE_SECONDS * 2);
        assert_eq!(outbox_retry_delay(i32::MAX), 30);
    }

    #[test]
    fn outbox_error_metadata_is_bounded() {
        let message = "x".repeat(OUTBOX_ERROR_LIMIT + 1);
        assert_eq!(bounded_outbox_error(&message).len(), OUTBOX_ERROR_LIMIT);
    }

    #[test]
    fn transient_outbox_failures_retry_twice_then_terminalize() {
        assert_eq!(outbox_failure_status(1, "messaging_unavailable"), "queued");
        assert_eq!(outbox_failure_status(2, "messaging_timeout"), "queued");
        assert_eq!(
            outbox_failure_status(OUTBOX_MAX_ATTEMPTS, "messaging_rejected"),
            "failed"
        );
        assert_eq!(
            outbox_failure_status(OUTBOX_MAX_ATTEMPTS + 1, "messaging_unavailable"),
            "failed"
        );
    }

    #[test]
    fn non_transient_outbox_failures_terminalize_without_retry() {
        for error_code in [
            "event_protocol_error",
            "event_encode_failed",
            "stored_propagation_invalid",
            "messaging_unroutable",
            "messaging_confirm_unavailable",
            "unknown_error",
        ] {
            assert_eq!(
                outbox_failure_status(1, error_code),
                "failed",
                "{error_code}"
            );
        }
    }

    #[tokio::test]
    async fn worker_handles_join_tasks_after_shared_cancellation() {
        let (shutdown_sender, mut shutdown_receiver) = watch::channel(false);
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_completed = completed.clone();
        let worker = tokio::spawn(async move {
            while !*shutdown_receiver.borrow() {
                if shutdown_receiver.changed().await.is_err() {
                    return;
                }
            }
            task_completed.store(true, Ordering::Release);
        });
        let workers = WorkerHandles {
            workers: vec![("test worker", worker)],
        };

        shutdown_sender
            .send(true)
            .expect("test shutdown signal sends");
        workers
            .shutdown()
            .await
            .expect("worker joins after cancellation");

        assert!(completed.load(Ordering::Acquire));
    }
}
