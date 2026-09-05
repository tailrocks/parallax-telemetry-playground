use anyhow::{Context as _, anyhow};
use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use futures_lite::StreamExt;
use lapin::{
    BasicProperties, Connection, ConnectionProperties, ExchangeKind,
    options::*,
    publisher_confirm::Confirmation,
    types::{AMQPValue, FieldTable},
};
use opentelemetry::baggage::{BaggageExt, KeyValueMetadata};
use opentelemetry::context::FutureExt as _;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::TraceContextExt;
use opentelemetry::{Context, global};
use playground_telemetry::semconv;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{RwLock, watch};
use tokio_postgres::NoTls;
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub(crate) const DEFAULT_RABBITMQ_URL: &str = "amqp://guest:guest@rabbitmq:5672/%2f";
const DEFAULT_DATABASE_URL: &str = "postgres://postgres:playground@postgres:5432/playground";
// This endpoint emits a synthetic dispatch contract, not the versioned
// CommerceEvent consumed by fulfillment. Keep it on a private exchange so an
// incompatible payload cannot enter the shared business-event namespace.
pub(crate) const SYNTHETIC_EXCHANGE: &str = "orders.synthetic";
pub(crate) const DEAD_LETTER_EXCHANGE: &str = "commerce.dlx";
pub(crate) const QUEUE: &str = "orders.dispatch";
pub(crate) const DEAD_QUEUE: &str = "orders.dispatch.dead";
pub(crate) const ROUTING_KEY: &str = "order.requested";
pub(crate) const DEAD_LETTER_ROUTING_KEY: &str = "order.dead";
pub(crate) const MAX_ATTEMPTS: u32 = 3;
pub(crate) const PREFETCH_COUNT: u16 = 1;

const DATABASE_MAX_CONNECTIONS: usize = 4;
const DATABASE_WAIT_TIMEOUT: Duration = Duration::from_secs(2);
const RABBIT_OPERATION_TIMEOUT: Duration = Duration::from_secs(5);
const RECONNECT_DELAY_INITIAL: Duration = Duration::from_secs(1);
const RECONNECT_DELAY_MAX: Duration = Duration::from_secs(30);
const MAX_RECONNECT_FAILURES: u32 = 10;
const MAX_TRACESTATE_HEADER_BYTES: usize = 512;
const MAX_TRACESTATE_MEMBERS: usize = 32;
const MAX_TRACESTATE_MEMBER_BYTES: usize = 256;
const MAX_BAGGAGE_HEADER_BYTES: usize = 2_048;
const MAX_BAGGAGE_MEMBERS: usize = 32;
const MAX_ENCODED_BAGGAGE_VALUE_BYTES: usize = 384;
const DEFAULT_TRACESTATE: &str = "playground=commerce";

#[derive(Clone)]
pub(crate) struct App {
    pub(crate) channel: Arc<RwLock<lapin::Channel>>,
    pub(crate) inbox: Arc<InboxStore>,
    pub(crate) consumer_ready: Arc<AtomicBool>,
    rabbitmq_url: Arc<String>,
    _connection: Arc<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OrderMessage {
    pub(crate) event_id: String,
    pub(crate) order_id: String,
    pub(crate) tenant_id: String,
    pub(crate) customer_id: String,
    pub(crate) event_type: String,
    pub(crate) poison: bool,
    pub(crate) lag_ms: u64,
    pub(crate) attempt: u32,
}

#[derive(Debug)]
struct PublishError {
    error: anyhow::Error,
    outcome_unknown: bool,
}

impl PublishError {
    fn known(error: anyhow::Error) -> Self {
        Self {
            error,
            outcome_unknown: false,
        }
    }

    fn unknown(error: anyhow::Error) -> Self {
        Self {
            error,
            outcome_unknown: true,
        }
    }
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

pub(crate) async fn publish_confirm(
    channel: &lapin::Channel,
    routing_key: &str,
    payload: &[u8],
    headers: FieldTable,
    message_id: &str,
    exchange: &str,
) -> anyhow::Result<()> {
    publish_confirm_with_outcome(channel, routing_key, payload, headers, message_id, exchange)
        .await
        .map_err(anyhow::Error::new)
}

async fn publish_confirm_with_outcome(
    channel: &lapin::Channel,
    routing_key: &str,
    payload: &[u8],
    headers: FieldTable,
    message_id: &str,
    exchange: &str,
) -> Result<(), PublishError> {
    validate_propagation_headers(&headers).map_err(|error| {
        PublishError::known(error.context("invalid RabbitMQ propagation headers"))
    })?;
    let publisher_confirm = tokio::time::timeout(
        RABBIT_OPERATION_TIMEOUT,
        channel.basic_publish(
            exchange,
            routing_key,
            mandatory_publish_options(),
            payload,
            BasicProperties::default()
                .with_content_type("application/json".into())
                .with_delivery_mode(2)
                .with_message_id(message_id.to_owned().into())
                .with_headers(headers),
        ),
    )
    .await
    .map_err(|_| PublishError::unknown(anyhow!("RabbitMQ publish timed out")))?
    .map_err(|error| PublishError::unknown(anyhow::Error::new(error)))?;

    let confirmation = tokio::time::timeout(RABBIT_OPERATION_TIMEOUT, publisher_confirm)
        .await
        .map_err(|_| PublishError::unknown(anyhow!("RabbitMQ publisher confirmation timed out")))?
        .map_err(|error| PublishError::unknown(anyhow::Error::new(error)))?;
    require_routed_ack(confirmation).map_err(PublishError::known)
}

pub(crate) async fn consume_loop(
    state: App,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let initial_channel = state.channel.read().await.clone();
    let mut broker = ConnectedBroker {
        channel: initial_channel,
        _connection: state._connection.clone(),
    };
    let mut reconnect_delay = RECONNECT_DELAY_INITIAL;
    let mut reconnect_failures = 0;

    loop {
        if *shutdown.borrow() {
            break;
        }
        let error = match consume_once(&state, &broker.channel, &mut shutdown).await {
            Ok(true) => break,
            Ok(false) => anyhow!("orders consumer stopped without an error"),
            Err(error) => error,
        };
        state.consumer_ready.store(false, Ordering::Release);
        reconnect_failures += 1;
        tracing::error!(
            error = %error,
            reconnect_failures,
            "orders consumer lost RabbitMQ subscription"
        );
        if reconnect_failures >= MAX_RECONNECT_FAILURES {
            return Err(error.context("orders consumer exceeded RabbitMQ reconnect budget"));
        }

        if wait_for_consumer_shutdown(&mut shutdown, reconnect_delay).await {
            break;
        }
        let reconnect = tokio::time::timeout(
            RABBIT_OPERATION_TIMEOUT,
            connect_broker(&state.rabbitmq_url),
        );
        tokio::pin!(reconnect);
        let result = tokio::select! {
            result = &mut reconnect => result,
            changed = shutdown.changed() => {
                let _ = changed;
                break;
            }
        };
        match result {
            Ok(Ok(next_broker)) => {
                *state.channel.write().await = next_broker.channel.clone();
                broker = next_broker;
                reconnect_delay = RECONNECT_DELAY_INITIAL;
                reconnect_failures = 0;
                tracing::info!("orders consumer RabbitMQ subscription reconnecting");
            }
            Ok(Err(connect_error)) => {
                tracing::error!(
                    error = %connect_error,
                    "orders consumer RabbitMQ reconnect failed"
                );
                reconnect_delay = next_reconnect_delay(reconnect_delay);
            }
            Err(connect_error) => {
                tracing::error!(
                    error = %connect_error,
                    "orders consumer RabbitMQ reconnect timed out"
                );
                reconnect_delay = next_reconnect_delay(reconnect_delay);
            }
        }
    }
    state.consumer_ready.store(false, Ordering::Release);
    Ok(())
}

async fn wait_for_consumer_shutdown(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        changed = shutdown.changed() => changed.is_err() || *shutdown.borrow(),
        _ = tokio::time::sleep(delay) => false,
    }
}

async fn consume_once(
    state: &App,
    channel: &lapin::Channel,
    shutdown: &mut watch::Receiver<bool>,
) -> anyhow::Result<bool> {
    tokio::time::timeout(
        RABBIT_OPERATION_TIMEOUT,
        channel.basic_qos(PREFETCH_COUNT, BasicQosOptions { global: false }),
    )
    .await
    .context("RabbitMQ consumer prefetch configuration timed out")??;

    let mut consumer = tokio::time::timeout(
        RABBIT_OPERATION_TIMEOUT,
        channel.basic_consume(
            QUEUE,
            "orders-service",
            BasicConsumeOptions::default(),
            FieldTable::default(),
        ),
    )
    .await
    .context("RabbitMQ consumer subscription timed out")??;
    state.consumer_ready.store(true, Ordering::Release);

    loop {
        if *shutdown.borrow() {
            state.consumer_ready.store(false, Ordering::Release);
            return Ok(true);
        }
        let delivery_result = tokio::select! {
            result = consumer.next() => result,
            changed = shutdown.changed() => {
                let _ = changed;
                state.consumer_ready.store(false, Ordering::Release);
                return Ok(true);
            }
        };
        let Some(delivery_result) = delivery_result else {
            break;
        };
        let delivery = match delivery_result {
            Ok(delivery) => delivery,
            Err(error) => {
                state.consumer_ready.store(false, Ordering::Release);
                return Err(anyhow::Error::new(error).context("RabbitMQ delivery stream failed"));
            }
        };
        let headers = delivery.properties.headers().clone().unwrap_or_default();
        let (inbound_context, mut context_failure) = match extract_context(&headers) {
            Ok(context) => (context, None),
            Err(error) => (Context::new(), Some(error)),
        };
        let orphan = match orphan_marker(&headers) {
            Ok(value) => value,
            Err(error) => {
                context_failure.get_or_insert(error);
                false
            }
        };
        let delivery_id = delivery_message_id(&delivery);
        let span = tracing::info_span!(
            "orders.consume",
            otel.kind = semconv::SPAN_KIND_CONSUMER,
            "messaging.system" = "rabbitmq",
            "messaging.destination.name" = QUEUE,
            "messaging.operation.name" = "process",
            "messaging.message.id" = tracing::field::Empty,
            "messaging.delivery.attempt" = tracing::field::Empty,
            "messaging.orphan" = tracing::field::Empty,
            job.id = %delivery_id,
            job.type = semconv::JOB_TYPE_ORDER_DISPATCH,
        );
        span.record("messaging.orphan", orphan);
        if inbound_context.span().span_context().is_valid() {
            span.add_link(inbound_context.span().span_context().clone());
        }
        playground_telemetry::stamp_business_baggage(&span, &inbound_context);

        let result = match context_failure {
            Some(error) => Err(DeliveryFailure::invalid_context(error)),
            None => {
                run_under_inbound_context(
                    process_delivery(state, channel, &delivery, &headers, &inbound_context)
                        .instrument(span.clone()),
                    inbound_context.clone(),
                )
                .await
            }
        };
        if let Err(failure) = result {
            let order_id = failure
                .message
                .as_ref()
                .map(|message| message.order_id.as_str())
                .unwrap_or(delivery_id.as_str());
            tracing::error!(
                error = %failure,
                order_id,
                "orders consumer failed to process delivery"
            );
            let recovery = async {
                if failure.malformed {
                    if failure.dead_letter_without_republish {
                        recover_invalid_context(&delivery).await
                    } else {
                        recover_malformed_delivery(channel, &delivery, &headers, &delivery.data)
                            .await
                    }
                } else {
                    recover_failed_delivery(state, &delivery, &failure).await
                }
            };
            if let Err(recovery_error) =
                run_under_inbound_context(recovery.instrument(span), inbound_context.clone()).await
            {
                state.consumer_ready.store(false, Ordering::Release);
                return Err(recovery_error.context("orders consumer failed to settle delivery"));
            }
        }
    }

    state.consumer_ready.store(false, Ordering::Release);
    Err(anyhow!("RabbitMQ consumer was canceled"))
}

async fn run_under_inbound_context<F>(future: F, inbound_context: Context) -> F::Output
where
    F: Future,
{
    future.with_context(inbound_context).await
}

async fn process_delivery(
    state: &App,
    channel: &lapin::Channel,
    delivery: &lapin::message::Delivery,
    headers: &FieldTable,
    inbound_context: &Context,
) -> Result<(), DeliveryFailure> {
    let message: OrderMessage = serde_json::from_slice(&delivery.data)
        .map_err(|error| DeliveryFailure::malformed(anyhow::Error::new(error), None))?;
    let attempt = canonical_attempt(message.attempt, headers)
        .map_err(|error| DeliveryFailure::malformed(error, Some(message.clone())))?;
    validate_message(&message, delivery)
        .map_err(|error| DeliveryFailure::malformed(error, Some(message.clone())))?;
    tracing::Span::current().record("messaging.message.id", &message.event_id);
    tracing::Span::current().record("messaging.delivery.attempt", attempt);

    if message.lag_ms > 0 {
        tokio::time::sleep(Duration::from_millis(message.lag_ms.min(30_000))).await;
    }

    let claim = match state
        .inbox
        .claim(
            &message.tenant_id,
            &message.event_id,
            attempt,
            inbound_context,
        )
        .await
        .map_err(|error| DeliveryFailure::retryable(error, Some(message.clone()), None, false))?
    {
        ClaimOutcome::Claimed(claim) => claim,
        ClaimOutcome::AlreadyHandled => {
            ack_delivery(delivery).await.map_err(|error| {
                DeliveryFailure::ack_unknown(error, Some(message.clone()), None, false)
            })?;
            tracing::info!(event_id = %message.event_id, "duplicate order event acknowledged by inbox fence");
            return Ok(());
        }
        ClaimOutcome::InFlight => {
            return Err(DeliveryFailure::retryable(
                anyhow!("order event is currently owned by another consumer lease"),
                Some(message),
                None,
                false,
            ));
        }
    };

    if claim.attempts > MAX_ATTEMPTS {
        let mut dead_message = message.clone();
        dead_message.attempt = MAX_ATTEMPTS;
        let payload = serde_json::to_vec(&dead_message).map_err(|error| {
            DeliveryFailure::retryable(
                anyhow::Error::new(error),
                Some(message.clone()),
                Some(claim.clone()),
                false,
            )
        })?;
        publish_or_fail(
            channel,
            &payload,
            headers,
            MAX_ATTEMPTS,
            RepublishDestination {
                exchange: DEAD_LETTER_EXCHANGE,
                routing_key: DEAD_LETTER_ROUTING_KEY,
                kind: "dead-letter-after-redelivery-budget",
            },
            &message,
            &claim,
        )
        .await?;
        state
            .inbox
            .mark_dead_lettered(&claim, "durable redelivery budget exhausted")
            .await
            .map_err(|error| {
                DeliveryFailure::retryable_with_publication(
                    error,
                    Some(message.clone()),
                    Some(claim.clone()),
                )
            })?;
        ack_delivery(delivery).await.map_err(|error| {
            DeliveryFailure::ack_unknown(error, Some(message.clone()), Some(claim), true)
        })?;
        tracing::error!(event_id = %message.event_id, "order event dead-lettered after durable redelivery budget");
        return Ok(());
    }

    if message.poison {
        let next_attempt = claim.attempts.checked_add(1).ok_or_else(|| {
            DeliveryFailure::retryable(
                anyhow!("durable order attempt overflow"),
                Some(message.clone()),
                Some(claim.clone()),
                false,
            )
        })?;
        if next_attempt > MAX_ATTEMPTS {
            let mut dead_message = message.clone();
            dead_message.attempt = MAX_ATTEMPTS;
            let payload = serde_json::to_vec(&dead_message).map_err(|error| {
                DeliveryFailure::retryable(
                    anyhow::Error::new(error),
                    Some(message.clone()),
                    Some(claim.clone()),
                    false,
                )
            })?;
            publish_or_fail(
                channel,
                &payload,
                headers,
                MAX_ATTEMPTS,
                RepublishDestination {
                    exchange: DEAD_LETTER_EXCHANGE,
                    routing_key: DEAD_LETTER_ROUTING_KEY,
                    kind: "dead-letter",
                },
                &message,
                &claim,
            )
            .await?;
            state
                .inbox
                .mark_dead_lettered(&claim, "poison event exhausted durable attempts")
                .await
                .map_err(|error| {
                    DeliveryFailure::retryable_with_publication(
                        error,
                        Some(message.clone()),
                        Some(claim.clone()),
                    )
                })?;
            ack_delivery(delivery).await.map_err(|error| {
                DeliveryFailure::ack_unknown(error, Some(message.clone()), Some(claim), true)
            })?;
            tracing::error!(event_id = %message.event_id, "poison order event dead-lettered after bounded retries");
            return Ok(());
        }

        let mut retry_message = message.clone();
        retry_message.attempt = next_attempt;
        let payload = serde_json::to_vec(&retry_message).map_err(|error| {
            DeliveryFailure::retryable(
                anyhow::Error::new(error),
                Some(message.clone()),
                Some(claim.clone()),
                false,
            )
        })?;
        publish_or_fail(
            channel,
            &payload,
            headers,
            next_attempt,
            RepublishDestination {
                exchange: SYNTHETIC_EXCHANGE,
                routing_key: ROUTING_KEY,
                kind: "retry",
            },
            &message,
            &claim,
        )
        .await?;
        state
            .inbox
            .mark_retry_scheduled(&claim, "poison event retry confirmed")
            .await
            .map_err(|error| {
                DeliveryFailure::retryable_with_publication(
                    error,
                    Some(message.clone()),
                    Some(claim.clone()),
                )
            })?;
        ack_delivery(delivery).await.map_err(|error| {
            DeliveryFailure::ack_unknown(error, Some(message.clone()), Some(claim), true)
        })?;
        tracing::warn!(event_id = %message.event_id, attempt = next_attempt, "poison event retry confirmed");
        return Ok(());
    }

    playground_telemetry::emit_event(
        "order.consumed",
        &[
            ("order_id", message.order_id.clone()),
            ("tenant_id", message.tenant_id.clone()),
        ],
    );
    state.inbox.mark_completed(&claim).await.map_err(|error| {
        DeliveryFailure::retryable(error, Some(message.clone()), Some(claim.clone()), false)
    })?;
    ack_delivery(delivery).await.map_err(|error| {
        DeliveryFailure::ack_unknown(error, Some(message.clone()), Some(claim), false)
    })?;
    tracing::info!(event_id = %message.event_id, "order event consumed");
    Ok(())
}

async fn publish_or_fail(
    channel: &lapin::Channel,
    payload: &[u8],
    headers: &FieldTable,
    attempt: u32,
    destination: RepublishDestination,
    message: &OrderMessage,
    claim: &InboxClaim,
) -> Result<(), DeliveryFailure> {
    match republish(
        channel,
        payload,
        headers,
        &message.event_id,
        attempt,
        destination,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(error) => {
            let outcome_unknown = error.outcome_unknown;
            Err(DeliveryFailure::retryable_with_publication_state(
                anyhow::Error::new(error),
                Some(message.clone()),
                Some(claim.clone()),
                outcome_unknown,
            ))
        }
    }
}

async fn recover_failed_delivery(
    state: &App,
    delivery: &lapin::message::Delivery,
    failure: &DeliveryFailure,
) -> anyhow::Result<()> {
    if failure_requires_reconnect(failure) {
        return Err(anyhow!(
            "delivery outcome is unknown; reconnecting without republishing: {}",
            failure.error
        ));
    }

    let Some(claim) = failure.claim.as_ref() else {
        return Err(anyhow!(
            "durable inbox claim did not complete; reconnecting without settling: {}",
            failure.error
        ));
    };
    state.inbox.release_claim(claim, &failure.error).await?;
    nack_delivery(delivery, true).await
}

fn failure_requires_reconnect(failure: &DeliveryFailure) -> bool {
    failure.ack_outcome_unknown || failure.publication_outcome_unknown
}

async fn recover_malformed_delivery(
    channel: &lapin::Channel,
    delivery: &lapin::message::Delivery,
    headers: &FieldTable,
    payload: &[u8],
) -> anyhow::Result<()> {
    let message_id = delivery_message_id(delivery);
    republish(
        channel,
        payload,
        headers,
        &message_id,
        MAX_ATTEMPTS,
        RepublishDestination {
            exchange: DEAD_LETTER_EXCHANGE,
            routing_key: DEAD_LETTER_ROUTING_KEY,
            kind: "malformed-dead-letter",
        },
    )
    .await
    .map_err(anyhow::Error::new)?;
    ack_delivery(delivery).await
}

async fn recover_invalid_context(delivery: &lapin::message::Delivery) -> anyhow::Result<()> {
    nack_delivery(delivery, false).await
}

async fn ack_delivery(delivery: &lapin::message::Delivery) -> anyhow::Result<()> {
    tokio::time::timeout(
        RABBIT_OPERATION_TIMEOUT,
        delivery.ack(BasicAckOptions::default()),
    )
    .await
    .context("RabbitMQ delivery acknowledgement timed out with unknown outcome")?
    .map_err(|error| {
        anyhow::Error::new(error).context("RabbitMQ delivery acknowledgement failed")
    })?;
    Ok(())
}

async fn nack_delivery(delivery: &lapin::message::Delivery, requeue: bool) -> anyhow::Result<()> {
    tokio::time::timeout(
        RABBIT_OPERATION_TIMEOUT,
        delivery.nack(BasicNackOptions {
            requeue,
            ..Default::default()
        }),
    )
    .await
    .context("RabbitMQ delivery negative acknowledgement timed out")?
    .map_err(|error| {
        anyhow::Error::new(error).context("RabbitMQ delivery negative acknowledgement failed")
    })?;
    Ok(())
}

fn context_with_inbound_baggage(context: Context, inbound_context: &Context) -> Context {
    context.with_baggage(
        inbound_context
            .baggage()
            .iter()
            .map(|(key, (value, metadata))| {
                KeyValueMetadata::new(key.clone(), value.clone(), metadata.clone())
            }),
    )
}

async fn republish(
    channel: &lapin::Channel,
    payload: &[u8],
    inbound_headers: &FieldTable,
    message_id: &str,
    attempt: u32,
    destination: RepublishDestination,
) -> Result<(), PublishError> {
    let inbound_context = extract_context(inbound_headers).map_err(|error| {
        PublishError::known(error.context("invalid RabbitMQ propagation headers"))
    })?;
    let span = tracing::info_span!(
        "orders.republish",
        otel.kind = semconv::SPAN_KIND_PRODUCER,
        "messaging.system" = "rabbitmq",
        "messaging.destination.name" = destination.exchange,
        "messaging.destination.routing_key" = destination.routing_key,
        "messaging.operation.name" = "send",
        "messaging.message.id" = %message_id,
        "messaging.delivery.attempt" = attempt,
        "orders.republish.kind" = destination.kind,
    );
    if inbound_context.span().span_context().is_valid() {
        span.add_link(inbound_context.span().span_context().clone());
    }
    playground_telemetry::stamp_business_baggage(&span, &inbound_context);

    async move {
        let producer_context =
            context_with_inbound_baggage(tracing::Span::current().context(), &inbound_context);
        let mut headers =
            republish_headers(&producer_context, attempt).map_err(PublishError::known)?;
        if let Some(marker) = amqp_header_value(inbound_headers, "messaging.orphan") {
            headers.insert(
                "messaging.orphan".into(),
                AMQPValue::LongString(marker.trim().to_owned().into()),
            );
        }
        publish_confirm_with_outcome(
            channel,
            destination.routing_key,
            payload,
            headers,
            message_id,
            destination.exchange,
        )
        .await
    }
    .instrument(span)
    .await
}

#[derive(Clone, Copy)]
struct RepublishDestination {
    exchange: &'static str,
    routing_key: &'static str,
    kind: &'static str,
}

#[derive(Debug)]
struct DeliveryFailure {
    error: anyhow::Error,
    malformed: bool,
    dead_letter_without_republish: bool,
    message: Option<OrderMessage>,
    claim: Option<InboxClaim>,
    ack_outcome_unknown: bool,
    publication_outcome_unknown: bool,
}

impl DeliveryFailure {
    fn malformed(error: anyhow::Error, message: Option<OrderMessage>) -> Self {
        Self {
            error,
            malformed: true,
            dead_letter_without_republish: false,
            message,
            claim: None,
            ack_outcome_unknown: false,
            publication_outcome_unknown: false,
        }
    }

    fn retryable(
        error: anyhow::Error,
        message: Option<OrderMessage>,
        claim: Option<InboxClaim>,
        publication_outcome_unknown: bool,
    ) -> Self {
        Self {
            error,
            malformed: false,
            dead_letter_without_republish: false,
            message,
            claim,
            ack_outcome_unknown: false,
            publication_outcome_unknown,
        }
    }

    fn retryable_with_publication(
        error: anyhow::Error,
        message: Option<OrderMessage>,
        claim: Option<InboxClaim>,
    ) -> Self {
        Self::retryable(error, message, claim, true)
    }

    fn retryable_with_publication_state(
        error: anyhow::Error,
        message: Option<OrderMessage>,
        claim: Option<InboxClaim>,
        publication_outcome_unknown: bool,
    ) -> Self {
        Self::retryable(error, message, claim, publication_outcome_unknown)
    }

    fn ack_unknown(
        error: anyhow::Error,
        message: Option<OrderMessage>,
        claim: Option<InboxClaim>,
        publication_outcome_unknown: bool,
    ) -> Self {
        Self {
            error,
            malformed: false,
            dead_letter_without_republish: false,
            message,
            claim,
            ack_outcome_unknown: true,
            publication_outcome_unknown,
        }
    }

    fn invalid_context(error: anyhow::Error) -> Self {
        Self {
            error,
            malformed: true,
            dead_letter_without_republish: true,
            message: None,
            claim: None,
            ack_outcome_unknown: false,
            publication_outcome_unknown: false,
        }
    }
}

impl fmt::Display for DeliveryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for DeliveryFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

pub(crate) struct InboxStore {
    pool: Pool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InboxClaim {
    tenant_id: String,
    event_id: String,
    attempts: u32,
}

#[derive(Debug, PartialEq, Eq)]
enum ClaimOutcome {
    Claimed(InboxClaim),
    AlreadyHandled,
    InFlight,
}

impl InboxStore {
    async fn connect() -> anyhow::Result<Self> {
        let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
        let pool = postgres_pool(&url)?;
        let store = Self { pool };
        let client = store.acquire().await?;
        client
            .query_one("SELECT 1", &[])
            .await
            .context("orders durable inbox database readiness")?;
        client
            .query_one("SELECT count(*) FROM orders_consumer_inbox", &[])
            .await
            .context("orders durable inbox schema missing; run deploy/postgres/migrate.sh")?;
        tracing::info!("orders connected to durable Postgres consumer inbox");
        Ok(store)
    }

    pub(crate) async fn health(&self) -> bool {
        let client = match tokio::time::timeout(DATABASE_WAIT_TIMEOUT, self.pool.get()).await {
            Ok(Ok(client)) => client,
            _ => return false,
        };
        client.query_one("SELECT 1", &[]).await.is_ok()
    }

    async fn acquire(&self) -> anyhow::Result<deadpool_postgres::Client> {
        tokio::time::timeout(DATABASE_WAIT_TIMEOUT, self.pool.get())
            .await
            .context("orders durable inbox database pool wait timed out")?
            .context("acquire orders durable inbox database connection")
    }

    async fn claim(
        &self,
        tenant_id: &str,
        event_id: &str,
        incoming_attempt: u32,
        context: &Context,
    ) -> anyhow::Result<ClaimOutcome> {
        let incoming_attempt = i32::try_from(incoming_attempt)
            .map_err(|_| anyhow!("order attempt does not fit PostgreSQL integer"))?;
        let (traceparent, tracestate, baggage) = context_carrier(context);
        let mut client = self.acquire().await?;
        let transaction = client.transaction().await?;
        let row = transaction
            .query_opt(
                r#"
                INSERT INTO orders_consumer_inbox
                    (tenant_id, event_id, status, attempts, lease_until,
                     traceparent, tracestate, baggage)
                VALUES
                    ($1, $2, 'processing', $3,
                     CURRENT_TIMESTAMP + INTERVAL '30 seconds', $4, $5, $6)
                ON CONFLICT (tenant_id, event_id) DO UPDATE
                SET status = 'processing',
                    attempts = CASE
                        WHEN EXCLUDED.attempts > orders_consumer_inbox.attempts
                            THEN EXCLUDED.attempts
                        ELSE orders_consumer_inbox.attempts + 1
                    END,
                    lease_until = CURRENT_TIMESTAMP + INTERVAL '30 seconds',
                    traceparent = COALESCE(EXCLUDED.traceparent, orders_consumer_inbox.traceparent),
                    tracestate = COALESCE(EXCLUDED.tracestate, orders_consumer_inbox.tracestate),
                    baggage = COALESCE(EXCLUDED.baggage, orders_consumer_inbox.baggage),
                    last_error = NULL,
                    updated_at = CURRENT_TIMESTAMP
                WHERE orders_consumer_inbox.status NOT IN ('completed', 'dead_lettered')
                  AND (
                      (orders_consumer_inbox.status = 'retry_scheduled'
                       AND EXCLUDED.attempts > orders_consumer_inbox.attempts)
                      OR (orders_consumer_inbox.status = 'processing'
                          AND orders_consumer_inbox.lease_until <= CURRENT_TIMESTAMP)
                      OR (orders_consumer_inbox.status = 'processing'
                          AND EXCLUDED.attempts > orders_consumer_inbox.attempts)
                  )
                RETURNING attempts
                "#,
                &[
                    &tenant_id,
                    &event_id,
                    &incoming_attempt,
                    &traceparent,
                    &tracestate,
                    &baggage,
                ],
            )
            .await?;

        if let Some(row) = row {
            let attempts = u32::try_from(row.get::<_, i32>(0))
                .map_err(|_| anyhow!("durable order attempt is negative"))?;
            transaction.commit().await?;
            return Ok(ClaimOutcome::Claimed(InboxClaim {
                tenant_id: tenant_id.to_owned(),
                event_id: event_id.to_owned(),
                attempts,
            }));
        }

        let row = transaction
            .query_opt(
                "SELECT status FROM orders_consumer_inbox WHERE tenant_id = $1 AND event_id = $2 FOR UPDATE",
                &[&tenant_id, &event_id],
            )
            .await?
            .ok_or_else(|| anyhow!("durable inbox claim disappeared"))?;
        let status: String = row.get(0);
        transaction.commit().await?;
        match status.as_str() {
            "completed" | "dead_lettered" | "retry_scheduled" => Ok(ClaimOutcome::AlreadyHandled),
            "processing" => Ok(ClaimOutcome::InFlight),
            _ => Err(anyhow!("durable inbox has unknown status {status}")),
        }
    }

    async fn mark_completed(&self, claim: &InboxClaim) -> anyhow::Result<()> {
        let client = self.acquire().await?;
        let attempts = claim.attempts_as_i32()?;
        let affected = client
            .execute(
                "UPDATE orders_consumer_inbox SET status = 'completed', completed_at = CURRENT_TIMESTAMP, lease_until = CURRENT_TIMESTAMP, last_error = NULL, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $1 AND event_id = $2 AND attempts = $3 AND status = 'processing'",
                &[&claim.tenant_id, &claim.event_id, &attempts],
            )
            .await?;
        require_one_row(affected, "complete durable order inbox claim")
    }

    async fn mark_retry_scheduled(&self, claim: &InboxClaim, reason: &str) -> anyhow::Result<()> {
        let client = self.acquire().await?;
        let attempts = claim.attempts_as_i32()?;
        let reason = bounded_text(reason);
        let affected = client
            .execute(
                "UPDATE orders_consumer_inbox SET status = 'retry_scheduled', lease_until = CURRENT_TIMESTAMP, last_error = $4, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $1 AND event_id = $2 AND attempts = $3 AND status = 'processing'",
                &[&claim.tenant_id, &claim.event_id, &attempts, &reason],
            )
            .await?;
        require_one_row(affected, "schedule durable order inbox retry")
    }

    async fn mark_dead_lettered(&self, claim: &InboxClaim, reason: &str) -> anyhow::Result<()> {
        let client = self.acquire().await?;
        let attempts = claim.attempts_as_i32()?;
        let reason = bounded_text(reason);
        let affected = client
            .execute(
                "UPDATE orders_consumer_inbox SET status = 'dead_lettered', lease_until = CURRENT_TIMESTAMP, last_error = $4, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $1 AND event_id = $2 AND attempts = $3 AND status = 'processing'",
                &[&claim.tenant_id, &claim.event_id, &attempts, &reason],
            )
            .await?;
        require_one_row(affected, "mark durable order inbox dead-lettered")
    }

    async fn release_claim(&self, claim: &InboxClaim, error: &anyhow::Error) -> anyhow::Result<()> {
        let client = self.acquire().await?;
        let attempts = claim.attempts_as_i32()?;
        let error = bounded_text(&error.to_string());
        client
            .execute(
                "UPDATE orders_consumer_inbox SET lease_until = CURRENT_TIMESTAMP, last_error = $4, updated_at = CURRENT_TIMESTAMP WHERE tenant_id = $1 AND event_id = $2 AND attempts = $3 AND status = 'processing'",
                &[&claim.tenant_id, &claim.event_id, &attempts, &error],
            )
            .await?;
        Ok(())
    }
}

impl InboxClaim {
    fn attempts_as_i32(&self) -> anyhow::Result<i32> {
        i32::try_from(self.attempts).map_err(|_| anyhow!("durable order attempt overflow"))
    }
}

fn require_one_row(affected: u64, operation: &str) -> anyhow::Result<()> {
    if affected == 1 {
        Ok(())
    } else {
        Err(anyhow!("{operation} affected {affected} rows"))
    }
}

fn bounded_text(value: &str) -> String {
    value.chars().take(1024).collect()
}

fn postgres_pool(database_url: &str) -> anyhow::Result<Pool> {
    let mut config = Config::new();
    config.url = Some(database_url.to_owned());
    config.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    config.pool = Some(deadpool_postgres::PoolConfig {
        max_size: DATABASE_MAX_CONNECTIONS,
        timeouts: deadpool_postgres::Timeouts {
            wait: Some(DATABASE_WAIT_TIMEOUT),
            create: Some(DATABASE_WAIT_TIMEOUT),
            recycle: Some(DATABASE_WAIT_TIMEOUT),
        },
        ..Default::default()
    });
    config
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .context("create orders durable inbox Postgres pool")
}

struct ConnectedBroker {
    channel: lapin::Channel,
    _connection: Arc<Connection>,
}

async fn connect_broker(url: &str) -> anyhow::Result<ConnectedBroker> {
    let connection = Arc::new(Connection::connect(url, ConnectionProperties::default()).await?);
    let channel = connection.create_channel().await?;
    channel
        .confirm_select(ConfirmSelectOptions::default())
        .await?;
    declare_topology(&channel).await?;
    Ok(ConnectedBroker {
        channel,
        _connection: connection,
    })
}

async fn declare_topology(channel: &lapin::Channel) -> anyhow::Result<()> {
    channel
        .exchange_declare(
            SYNTHETIC_EXCHANGE,
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;
    channel
        .exchange_declare(
            DEAD_LETTER_EXCHANGE,
            ExchangeKind::Topic,
            ExchangeDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;
    channel
        .queue_declare(
            QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            dead_letter_queue_arguments(),
        )
        .await?;
    channel
        .queue_declare(
            DEAD_QUEUE,
            QueueDeclareOptions {
                durable: true,
                ..Default::default()
            },
            FieldTable::default(),
        )
        .await?;
    channel
        .queue_bind(
            QUEUE,
            SYNTHETIC_EXCHANGE,
            ROUTING_KEY,
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await?;
    channel
        .queue_bind(
            DEAD_QUEUE,
            DEAD_LETTER_EXCHANGE,
            DEAD_LETTER_ROUTING_KEY,
            QueueBindOptions::default(),
            FieldTable::default(),
        )
        .await?;
    Ok(())
}

pub(crate) async fn broker() -> anyhow::Result<App> {
    let url = std::env::var("RABBITMQ_URL").unwrap_or_else(|_| DEFAULT_RABBITMQ_URL.to_owned());
    let broker = connect_broker(&url).await?;
    let inbox = Arc::new(InboxStore::connect().await?);
    Ok(App {
        channel: Arc::new(RwLock::new(broker.channel)),
        inbox,
        consumer_ready: Arc::new(AtomicBool::new(false)),
        rabbitmq_url: Arc::new(url),
        _connection: broker._connection,
    })
}

fn next_reconnect_delay(current: Duration) -> Duration {
    if current >= RECONNECT_DELAY_MAX / 2 {
        RECONNECT_DELAY_MAX
    } else {
        current + current
    }
}

fn validate_message(
    message: &OrderMessage,
    delivery: &lapin::message::Delivery,
) -> anyhow::Result<()> {
    if message.event_id.trim().is_empty() {
        return Err(anyhow!("order event_id is empty"));
    }
    if message.order_id.trim().is_empty() {
        return Err(anyhow!("order order_id is empty"));
    }
    if message.tenant_id.trim().is_empty() {
        return Err(anyhow!("order tenant_id is empty"));
    }
    if message.customer_id.trim().is_empty() {
        return Err(anyhow!("order customer_id is empty"));
    }
    if let Some(message_id) = delivery.properties.message_id().as_ref()
        && message_id.as_str() != message.event_id
    {
        return Err(anyhow!("RabbitMQ message_id does not match order event_id"));
    }
    Ok(())
}

fn canonical_attempt(message_attempt: u32, headers: &FieldTable) -> anyhow::Result<u32> {
    if message_attempt == 0 {
        return Err(anyhow!("order attempt must be positive"));
    }
    if message_attempt > MAX_ATTEMPTS {
        return Err(anyhow!("order attempt exceeds MAX_ATTEMPTS"));
    }
    let Some(header_attempt) = header_attempt(headers)? else {
        i32::try_from(message_attempt)
            .map_err(|_| anyhow!("order attempt does not fit PostgreSQL integer"))?;
        return Ok(message_attempt);
    };
    if header_attempt != message_attempt {
        return Err(anyhow!("order body/header attempts disagree"));
    }
    i32::try_from(message_attempt)
        .map_err(|_| anyhow!("order attempt does not fit PostgreSQL integer"))?;
    Ok(message_attempt)
}

fn header_attempt(headers: &FieldTable) -> anyhow::Result<Option<u32>> {
    let Some(value) = headers.inner().get("x-order-attempt") else {
        return Ok(None);
    };
    match value {
        AMQPValue::LongUInt(value) if *value > 0 => Ok(Some(*value)),
        AMQPValue::LongLongInt(value) => u32::try_from(*value)
            .ok()
            .filter(|value| *value > 0)
            .map(Some)
            .ok_or_else(|| anyhow!("x-order-attempt is outside the positive u32 range")),
        _ => Err(anyhow!("x-order-attempt has an invalid AMQP type or value")),
    }
}

fn delivery_message_id(delivery: &lapin::message::Delivery) -> String {
    delivery
        .properties
        .message_id()
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("delivery-{}", delivery.delivery_tag))
}

fn context_carrier(context: &Context) -> (Option<String>, Option<String>, Option<String>) {
    let mut headers = FieldTable::default();
    inject_context(context, &mut headers);
    (
        amqp_header_value(&headers, "traceparent"),
        amqp_header_value(&headers, "tracestate"),
        amqp_header_value(&headers, "baggage"),
    )
}

struct AmqpInjector<'a>(&'a mut FieldTable);

impl Injector for AmqpInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0
            .insert(key.into(), AMQPValue::LongString(value.into()));
    }
}

struct AmqpExtractor<'a>(&'a FieldTable);

impl Extractor for AmqpExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.inner().get(key).and_then(|value| match value {
            AMQPValue::LongString(value) => std::str::from_utf8(value.as_bytes()).ok(),
            AMQPValue::ShortString(value) => Some(value.as_str()),
            _ => None,
        })
    }

    fn keys(&self) -> Vec<&str> {
        self.0.inner().keys().map(|key| key.as_str()).collect()
    }
}

pub(crate) fn inject_context(context: &Context, headers: &mut FieldTable) {
    let context = playground_telemetry::sanitize_context(context);
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut AmqpInjector(headers))
    });
    if let Some(traceparent) = amqp_header_value(headers, "traceparent")
        && !traceparent.trim().is_empty()
        && amqp_header_value(headers, "tracestate").is_none_or(|value| value.trim().is_empty())
    {
        headers.insert(
            "tracestate".into(),
            AMQPValue::LongString(DEFAULT_TRACESTATE.into()),
        );
    }
}

fn orphan_marker(headers: &FieldTable) -> anyhow::Result<bool> {
    let Some(value) = amqp_header_value(headers, "messaging.orphan") else {
        return Ok(false);
    };
    match value.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(anyhow!("RabbitMQ messaging.orphan header is invalid")),
    }
}

fn extract_context(headers: &FieldTable) -> anyhow::Result<Context> {
    validate_propagation_headers(headers)?;
    let context =
        global::get_text_map_propagator(|propagator| propagator.extract(&AmqpExtractor(headers)));
    let context = playground_telemetry::sanitize_context(&context);
    if !context.span().span_context().is_valid() {
        return Err(anyhow!("RabbitMQ traceparent could not be extracted"));
    }
    Ok(context)
}

fn amqp_header_value(headers: &FieldTable, key: &str) -> Option<String> {
    headers.inner().get(key).and_then(|value| match value {
        AMQPValue::LongString(value) => std::str::from_utf8(value.as_bytes())
            .ok()
            .map(str::to_owned),
        AMQPValue::ShortString(value) => Some(value.to_string()),
        _ => None,
    })
}

fn republish_headers(context: &Context, attempt: u32) -> anyhow::Result<FieldTable> {
    let mut headers = FieldTable::default();
    inject_context(context, &mut headers);
    validate_propagation_headers(&headers)?;
    headers.insert("x-order-attempt".into(), AMQPValue::LongUInt(attempt));
    Ok(headers)
}

fn validate_propagation_headers(headers: &FieldTable) -> anyhow::Result<()> {
    let traceparent = required_amqp_header(headers, "traceparent")?;
    let tracestate = required_amqp_header(headers, "tracestate")?;
    let baggage = required_amqp_header(headers, "baggage")?;
    validate_traceparent(&traceparent)?;
    validate_tracestate(&tracestate)?;
    validate_baggage(&baggage)?;
    Ok(())
}

fn required_amqp_header(headers: &FieldTable, key: &str) -> anyhow::Result<String> {
    amqp_header_value(headers, key)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("RabbitMQ {key} header is required"))
}

fn validate_traceparent(value: &str) -> anyhow::Result<()> {
    let bytes = value.as_bytes();
    if bytes.len() != 55 || &bytes[0..3] != b"00-" || bytes[35] != b'-' || bytes[52] != b'-' {
        return Err(anyhow!("RabbitMQ traceparent header is invalid"));
    }
    if !bytes[3..35].iter().copied().all(is_lower_hex)
        || !bytes[36..52].iter().copied().all(is_lower_hex)
        || !bytes[53..55].iter().copied().all(is_lower_hex)
        || bytes[3..35].iter().all(|byte| *byte == b'0')
        || bytes[36..52].iter().all(|byte| *byte == b'0')
    {
        return Err(anyhow!("RabbitMQ traceparent header is invalid"));
    }
    Ok(())
}

fn validate_tracestate(value: &str) -> anyhow::Result<()> {
    if value.len() > MAX_TRACESTATE_HEADER_BYTES {
        return Err(anyhow!("RabbitMQ tracestate header is too large"));
    }
    let members = value.split(',').collect::<Vec<_>>();
    if members.is_empty() || members.len() > MAX_TRACESTATE_MEMBERS {
        return Err(anyhow!("RabbitMQ tracestate header has invalid members"));
    }
    let mut seen = Vec::with_capacity(members.len());
    for raw_member in members {
        let member = trim_ows(raw_member);
        let Some((key, member_value)) = member.split_once('=') else {
            return Err(anyhow!("RabbitMQ tracestate header has an invalid member"));
        };
        if member.is_empty()
            || key.is_empty()
            || member_value.is_empty()
            || member_value.contains('=')
            || member.len() > MAX_TRACESTATE_MEMBER_BYTES
            || !valid_tracestate_key(key)
            || member_value.len() > MAX_TRACESTATE_MEMBER_BYTES
            || !valid_tracestate_value(member_value)
            || seen.iter().any(|known| known == &key)
        {
            return Err(anyhow!("RabbitMQ tracestate header has an invalid member"));
        }
        seen.push(key);
    }
    Ok(())
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
            .is_some_and(is_lower_alpha_numeric)
        && value
            .as_bytes()
            .iter()
            .skip(1)
            .copied()
            .all(|byte| is_lower_alpha_numeric(byte) || b"_*/-".contains(&byte))
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

fn validate_baggage(value: &str) -> anyhow::Result<()> {
    if value.len() > MAX_BAGGAGE_HEADER_BYTES {
        return Err(anyhow!("RabbitMQ baggage header is too large"));
    }
    let members = value.split(',').collect::<Vec<_>>();
    if members.is_empty() || members.len() > MAX_BAGGAGE_MEMBERS {
        return Err(anyhow!("RabbitMQ baggage header has invalid members"));
    }
    let mut seen = Vec::with_capacity(members.len());
    for raw_member in members {
        let member = trim_ows(raw_member);
        let Some((key, value_and_metadata)) = member.split_once('=') else {
            return Err(anyhow!("RabbitMQ baggage header has an invalid member"));
        };
        let key = trim_ows(key);
        let mut values = value_and_metadata.split(';');
        let encoded_value = trim_ows(values.next().unwrap_or_default());
        if member.is_empty()
            || !valid_baggage_key(key)
            || !valid_encoded_baggage_value(encoded_value)
            || seen.iter().any(|known| known == &key)
        {
            return Err(anyhow!("RabbitMQ baggage header has an invalid member"));
        }
        seen.push(key);
        for metadata in values {
            let metadata = trim_ows(metadata);
            let Some((metadata_key, metadata_value)) = metadata.split_once('=') else {
                return Err(anyhow!("RabbitMQ baggage header has an invalid member"));
            };
            if !valid_baggage_key(trim_ows(metadata_key))
                || !valid_encoded_baggage_value(trim_ows(metadata_value))
            {
                return Err(anyhow!("RabbitMQ baggage header has an invalid member"));
            }
        }
    }
    Ok(())
}

fn valid_baggage_key(value: &str) -> bool {
    let mut parts = value.split('@');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    parts.next().is_none()
        && valid_baggage_key_part(first, 256)
        && second.is_none_or(|part| valid_baggage_key_part(part, 14))
}

fn valid_baggage_key_part(value: &str, max_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_length
        && value
            .as_bytes()
            .first()
            .copied()
            .is_some_and(is_lower_alpha_numeric)
        && value
            .as_bytes()
            .iter()
            .skip(1)
            .copied()
            .all(|byte| is_lower_alpha_numeric(byte) || b"._-".contains(&byte))
}

fn valid_encoded_baggage_value(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_ENCODED_BAGGAGE_VALUE_BYTES {
        return false;
    }
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() || !is_hex(bytes[index + 1]) || !is_hex(bytes[index + 2]) {
                return false;
            }
            index += 3;
            continue;
        }
        if !((0x21..=0x2b).contains(&bytes[index])
            || (0x2d..=0x3c).contains(&bytes[index])
            || (0x3e..=0x7e).contains(&bytes[index]))
        {
            return false;
        }
        index += 1;
    }
    true
}

fn trim_ows(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}

fn is_lower_alpha_numeric(value: u8) -> bool {
    value.is_ascii_lowercase() || value.is_ascii_digit()
}

fn is_lower_hex(value: u8) -> bool {
    value.is_ascii_digit() || (b'a'..=b'f').contains(&value)
}

fn is_hex(value: u8) -> bool {
    value.is_ascii_digit() || (b'a'..=b'f').contains(&value) || (b'A'..=b'F').contains(&value)
}

fn mandatory_publish_options() -> BasicPublishOptions {
    BasicPublishOptions {
        mandatory: true,
        ..Default::default()
    }
}

fn require_routed_ack(confirmation: Confirmation) -> anyhow::Result<()> {
    match confirmation {
        Confirmation::Ack(None) => Ok(()),
        Confirmation::Ack(Some(returned)) => Err(anyhow!(
            "RabbitMQ returned a mandatory publication: {returned:?}"
        )),
        Confirmation::Nack(returned) => Err(anyhow!(
            "RabbitMQ negatively acknowledged publication: returned={returned:?}"
        )),
        Confirmation::NotRequested => Err(anyhow!("RabbitMQ publisher confirms are not enabled")),
    }
}

fn dead_letter_queue_arguments() -> FieldTable {
    let mut arguments = FieldTable::default();
    arguments.insert(
        "x-dead-letter-exchange".into(),
        AMQPValue::LongString(DEAD_LETTER_EXCHANGE.into()),
    );
    arguments.insert(
        "x-dead-letter-routing-key".into(),
        AMQPValue::LongString(DEAD_LETTER_ROUTING_KEY.into()),
    );
    arguments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::default_customer;
    use opentelemetry::baggage::{Baggage, BaggageExt};
    use opentelemetry::propagation::{TextMapPropagator, text_map_propagator::FieldIter};
    use opentelemetry::trace::{SpanContext, SpanId, TraceFlags, TraceId, TraceState};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    #[derive(Debug)]
    struct TestW3cPropagator;

    impl TextMapPropagator for TestW3cPropagator {
        fn inject_context(&self, context: &Context, injector: &mut dyn Injector) {
            let span = context.span();
            let span_context = span.span_context();
            if span_context.is_valid() {
                injector.set(
                    "traceparent",
                    format!(
                        "00-{}-{}-{:02x}",
                        span_context.trace_id(),
                        span_context.span_id(),
                        span_context.trace_flags().to_u8()
                    ),
                );
                let tracestate = span_context.trace_state().header();
                if !tracestate.is_empty() {
                    injector.set("tracestate", tracestate);
                }
            }
            let baggage = context.baggage().to_string();
            if !baggage.is_empty() {
                injector.set("baggage", baggage);
            }
        }

        fn extract_with_context(&self, _context: &Context, extractor: &dyn Extractor) -> Context {
            let context = extractor
                .get("traceparent")
                .and_then(parse_test_traceparent)
                .map(|span_context| Context::new().with_remote_span_context(span_context))
                .unwrap_or_default();
            let mut baggage = Baggage::new();
            if let Some(value) = extractor.get("baggage") {
                for member in value.split(',') {
                    let Some((key, value)) = member.trim().split_once('=') else {
                        continue;
                    };
                    let _ = baggage.insert_with_metadata(
                        key.trim().to_owned(),
                        value.trim().to_owned(),
                        "",
                    );
                }
            }
            context.with_baggage(baggage)
        }

        fn fields(&self) -> FieldIter<'_> {
            FieldIter::new(&[])
        }
    }

    fn parse_test_traceparent(value: &str) -> Option<SpanContext> {
        let mut fields = value.split('-');
        if fields.next()? != "00" {
            return None;
        }
        let trace_id = TraceId::from_hex(fields.next()?).ok()?;
        let span_id = SpanId::from_hex(fields.next()?).ok()?;
        let flags = u8::from_str_radix(fields.next()?, 16).ok()?;
        Some(SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::new(flags),
            true,
            TraceState::default(),
        ))
    }

    fn test_context(span_id: &str, tracestate: &str) -> Context {
        let trace_state =
            TraceState::from_key_value([("vendor", tracestate)]).expect("valid test tracestate");
        Context::new().with_remote_span_context(SpanContext::new(
            TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id"),
            SpanId::from_hex(span_id).expect("span id"),
            TraceFlags::SAMPLED,
            false,
            trace_state,
        ))
    }

    fn header_value(headers: &FieldTable, key: &str) -> Option<String> {
        amqp_header_value(headers, key)
    }

    fn propagator_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("propagator lock")
    }

    #[tokio::test]
    async fn consumer_work_keeps_extracted_baggage_current_across_awaits() {
        let inbound_context = {
            let _guard = propagator_lock();
            global::set_text_map_propagator(TestW3cPropagator);

            let mut headers = FieldTable::default();
            headers.insert(
                "traceparent".into(),
                AMQPValue::LongString(
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into(),
                ),
            );
            headers.insert(
                "tracestate".into(),
                AMQPValue::LongString("vendor=state".into()),
            );
            headers.insert(
                "baggage".into(),
                AMQPValue::LongString("tenant.id=tenant-acme,customer.segment=gold".into()),
            );
            extract_context(&headers).expect("valid inbound context")
        };

        let observed = run_under_inbound_context(
            async {
                tokio::task::yield_now().await;
                (
                    Context::current()
                        .baggage()
                        .get("tenant.id")
                        .map(ToString::to_string),
                    Context::current()
                        .baggage()
                        .get("customer.segment")
                        .map(ToString::to_string),
                )
            },
            inbound_context,
        )
        .await;

        assert_eq!(
            observed,
            (Some("tenant-acme".to_owned()), Some("gold".to_owned()))
        );
    }

    #[test]
    fn republish_headers_use_fresh_context_and_drop_hostile_baggage() {
        let _guard = propagator_lock();
        global::set_text_map_propagator(TestW3cPropagator);

        let mut inbound_headers = FieldTable::default();
        inbound_headers.insert(
            "traceparent".into(),
            AMQPValue::LongString("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into()),
        );
        inbound_headers.insert(
            "tracestate".into(),
            AMQPValue::LongString("vendor=state".into()),
        );
        inbound_headers.insert(
            "baggage".into(),
            AMQPValue::LongString(
                format!(
                    "tenant.id=tenant-acme,unknown.secret=do-not-forward,region={},customer.segment=gold",
                    "x".repeat(129)
                )
                .into(),
            ),
        );

        let inbound_context = extract_context(&inbound_headers).expect("valid inbound context");
        assert_eq!(
            inbound_context
                .baggage()
                .get("tenant.id")
                .map(ToString::to_string),
            Some("tenant-acme".to_owned())
        );
        assert!(inbound_context.baggage().get("unknown.secret").is_none());
        assert!(inbound_context.baggage().get("region").is_none());

        let first_context = context_with_inbound_baggage(
            test_context("0000000000000002", "first"),
            &inbound_context,
        );
        let second_context = context_with_inbound_baggage(
            test_context("0000000000000003", "second"),
            &inbound_context,
        );
        let first = republish_headers(&first_context, 2).expect("first propagation headers");
        let second = republish_headers(&second_context, 3).expect("second propagation headers");
        assert_ne!(
            header_value(&first, "traceparent"),
            header_value(&second, "traceparent")
        );
        assert!(
            header_value(&first, "traceparent")
                .expect("first traceparent")
                .ends_with("-0000000000000002-01")
        );
        assert!(
            header_value(&second, "traceparent")
                .expect("second traceparent")
                .ends_with("-0000000000000003-01")
        );
        assert_eq!(
            header_value(&first, "tracestate"),
            Some("vendor=first".to_owned())
        );
        assert_eq!(
            header_value(&second, "tracestate"),
            Some("vendor=second".to_owned())
        );
        assert_eq!(
            first.inner().get("x-order-attempt"),
            Some(&AMQPValue::LongUInt(2))
        );
        assert_eq!(
            second.inner().get("x-order-attempt"),
            Some(&AMQPValue::LongUInt(3))
        );
        let first_baggage = header_value(&first, "baggage").expect("safe baggage");
        assert!(first_baggage.contains("tenant.id=tenant-acme"));
        assert!(first_baggage.contains("customer.segment=gold"));
        assert!(!first_baggage.contains("unknown.secret"));
        assert!(!first_baggage.contains("region="));
    }

    #[test]
    fn attempts_are_canonical_checked_and_never_defaulted() {
        let mut headers = FieldTable::default();
        headers.insert("x-order-attempt".into(), AMQPValue::LongUInt(2));
        assert_eq!(canonical_attempt(2, &headers).expect("matching attempt"), 2);
        assert!(canonical_attempt(1, &headers).is_err());

        headers.insert(
            "x-order-attempt".into(),
            AMQPValue::LongLongInt(i64::from(u32::MAX) + 1),
        );
        assert!(header_attempt(&headers).is_err());
        headers.insert("x-order-attempt".into(), AMQPValue::LongLongInt(-1));
        assert!(header_attempt(&headers).is_err());
        headers.insert("x-order-attempt".into(), AMQPValue::LongUInt(0));
        assert!(header_attempt(&headers).is_err());
    }

    #[test]
    fn rabbit_event_boundary_requires_valid_complete_w3c_context() {
        let mut headers = FieldTable::default();
        headers.insert(
            "traceparent".into(),
            AMQPValue::LongString("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".into()),
        );
        headers.insert(
            "tracestate".into(),
            AMQPValue::LongString("vendor=state".into()),
        );
        headers.insert(
            "baggage".into(),
            AMQPValue::LongString("tenant.id=tenant-acme".into()),
        );
        assert!(validate_propagation_headers(&headers).is_ok());

        for key in ["traceparent", "tracestate", "baggage"] {
            let mut missing = FieldTable::default();
            for (name, value) in headers.inner() {
                if name.as_str() != key {
                    missing.insert(name.clone(), value.clone());
                }
            }
            assert!(
                validate_propagation_headers(&missing).is_err(),
                "missing {key}"
            );
        }

        let mut invalid = headers.clone();
        invalid.insert(
            "traceparent".into(),
            AMQPValue::LongString("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01".into()),
        );
        assert!(validate_propagation_headers(&invalid).is_err());

        let mut malformed_baggage = headers;
        malformed_baggage.insert(
            "baggage".into(),
            AMQPValue::LongString("tenant.id=%zz".into()),
        );
        assert!(validate_propagation_headers(&malformed_baggage).is_err());
    }

    #[test]
    fn rabbit_tracestate_matches_shared_w3c_member_limits_and_syntax() {
        for value in ["vendor/name=state", " vendor=state \t"] {
            assert!(
                validate_tracestate(value).is_ok(),
                "RabbitMQ tracestate must accept {value:?}"
            );
        }

        let max_member = format!("k={}", "v".repeat(MAX_TRACESTATE_MEMBER_BYTES - 2));
        assert_eq!(max_member.len(), MAX_TRACESTATE_MEMBER_BYTES);
        assert!(validate_tracestate(&max_member).is_ok());

        let too_long_member = format!("k={}", "v".repeat(MAX_TRACESTATE_MEMBER_BYTES - 1));
        assert_eq!(too_long_member.len(), MAX_TRACESTATE_MEMBER_BYTES + 1);
        assert!(validate_tracestate(&too_long_member).is_err());

        for value in [
            "vendor=state,",
            ",vendor=state",
            "vendor=state,,other=value",
            "k=",
            "k==state",
            "K=state",
            "vendor=state,vendor=other",
        ] {
            assert!(
                validate_tracestate(value).is_err(),
                "RabbitMQ tracestate must reject {value:?}"
            );
        }
    }

    #[test]
    fn injector_adds_local_tracestate_to_a_valid_root_context() {
        let _guard = propagator_lock();
        global::set_text_map_propagator(TestW3cPropagator);
        let context = Context::new()
            .with_remote_span_context(SpanContext::new(
                TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").expect("trace id"),
                SpanId::from_hex("00f067aa0ba902b7").expect("span id"),
                TraceFlags::SAMPLED,
                false,
                TraceState::default(),
            ))
            .with_baggage([opentelemetry::KeyValue::new("tenant.id", "tenant-acme")]);
        let mut headers = FieldTable::default();

        inject_context(&context, &mut headers);

        assert_eq!(
            header_value(&headers, "tracestate"),
            Some(DEFAULT_TRACESTATE.to_owned())
        );
        assert!(validate_propagation_headers(&headers).is_ok());
    }

    #[test]
    fn orphan_marker_accepts_only_explicit_boolean_values() {
        let mut headers = FieldTable::default();
        assert!(!orphan_marker(&headers).expect("missing marker is false"));
        headers.insert(
            "messaging.orphan".into(),
            AMQPValue::LongString("true".into()),
        );
        assert!(orphan_marker(&headers).expect("true marker"));
        headers.insert(
            "messaging.orphan".into(),
            AMQPValue::LongString("unexpected".into()),
        );
        assert!(orphan_marker(&headers).is_err());
    }

    #[test]
    fn invalid_context_is_dead_lettered_without_republish() {
        let failure = DeliveryFailure::invalid_context(anyhow!("missing baggage"));
        assert!(failure.malformed);
        assert!(failure.dead_letter_without_republish);
        assert!(!failure_requires_reconnect(&failure));
    }

    #[test]
    fn poison_retries_are_bounded_and_message_identity_is_stable() {
        let mut message = OrderMessage {
            event_id: "event-1".into(),
            order_id: "order-1".into(),
            tenant_id: "tenant-acme".into(),
            customer_id: default_customer(),
            event_type: ROUTING_KEY.into(),
            poison: true,
            lag_ms: 0,
            attempt: 1,
        };
        assert!(message.attempt < MAX_ATTEMPTS);
        message.attempt += 1;
        assert_eq!(message.event_id, "event-1");
        assert_eq!(message.attempt, 2);
    }

    #[test]
    fn synthetic_dispatch_contract_has_a_private_exchange() {
        assert_eq!(SYNTHETIC_EXCHANGE, "orders.synthetic");
        assert_ne!(SYNTHETIC_EXCHANGE, "commerce.events");
    }

    #[test]
    fn rabbit_consumer_budget_and_reconnect_backoff_are_bounded() {
        assert_eq!(PREFETCH_COUNT, 1);
        assert_eq!(
            next_reconnect_delay(RECONNECT_DELAY_INITIAL),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_reconnect_delay(RECONNECT_DELAY_MAX),
            RECONNECT_DELAY_MAX
        );
    }

    #[test]
    fn ambiguous_ack_requires_reconnect_without_republish() {
        let failure =
            DeliveryFailure::ack_unknown(anyhow!("ack outcome unknown"), None, None, true);
        assert!(failure_requires_reconnect(&failure));

        let failure = DeliveryFailure::retryable(anyhow!("publish rejected"), None, None, false);
        assert!(!failure_requires_reconnect(&failure));
    }

    #[test]
    fn publisher_requires_confirmed_routed_ack() {
        assert!(mandatory_publish_options().mandatory);
        assert!(require_routed_ack(Confirmation::Ack(None)).is_ok());
        assert!(require_routed_ack(Confirmation::Nack(None)).is_err());
        assert!(require_routed_ack(Confirmation::NotRequested).is_err());
    }

    #[test]
    fn main_queue_dead_letters_to_declared_route() {
        let arguments = dead_letter_queue_arguments();
        assert_eq!(
            arguments.inner().get("x-dead-letter-exchange"),
            Some(&AMQPValue::LongString(DEAD_LETTER_EXCHANGE.into()))
        );
        assert_eq!(
            arguments.inner().get("x-dead-letter-routing-key"),
            Some(&AMQPValue::LongString(DEAD_LETTER_ROUTING_KEY.into()))
        );
    }

    #[tokio::test]
    async fn consumer_reconnect_wait_is_interruptible() {
        let (shutdown_sender, mut shutdown_receiver) = watch::channel(false);
        let waiter = tokio::spawn(async move {
            wait_for_consumer_shutdown(&mut shutdown_receiver, Duration::from_secs(30)).await
        });

        shutdown_sender
            .send(true)
            .expect("test shutdown signal sends");
        assert!(
            tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("consumer wait exits after cancellation")
                .expect("consumer wait task joins")
        );
    }
}
