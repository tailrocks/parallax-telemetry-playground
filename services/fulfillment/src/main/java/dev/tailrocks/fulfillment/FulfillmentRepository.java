package dev.tailrocks.fulfillment;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import io.opentelemetry.api.trace.Span;
import java.nio.charset.StandardCharsets;
import java.sql.Timestamp;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.UUID;
import jakarta.annotation.PostConstruct;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.stereotype.Repository;
import org.springframework.transaction.support.TransactionTemplate;

@Repository
class FulfillmentRepository {
    static final String ORDER_CONSUMER = "fulfillment.orders";
    static final String ANALYTICS_CONSUMER = "fulfillment.analytics";
    static final int MAX_CLAIM_ATTEMPTS = 5;
    private static final Set<String> TERMINAL_ORDER_STATUSES = Set.of(
        "shipped",
        "delivered",
        "cancelled",
        "refunded"
    );
    private static final Set<String> NON_CANCELLABLE_ORDER_STATUSES = Set.of(
        "shipped",
        "delivered",
        "cancelled",
        "refunded"
    );
    private static final Set<String> TERMINAL_SHIPMENT_STATUSES = Set.of(
        "in_transit",
        "delivered",
        "cancelled",
        "returned"
    );

    private final JdbcTemplate jdbc;
    private final TransactionTemplate transactions;
    private final ObjectMapper mapper;
    private final ScheduledExecutorService leaseExecutor;

    @Value("${fulfillment.claim-lease-seconds:30}")
    private long claimLeaseSeconds = 30;

    @Autowired
    FulfillmentRepository(
        JdbcTemplate jdbc,
        TransactionTemplate transactions,
        ObjectMapper mapper,
        ScheduledExecutorService leaseExecutor
    ) {
        this.jdbc = jdbc;
        this.transactions = transactions;
        this.mapper = mapper;
        this.leaseExecutor = leaseExecutor;
    }

    FulfillmentRepository(
        JdbcTemplate jdbc,
        TransactionTemplate transactions,
        ObjectMapper mapper
    ) {
        this(jdbc, transactions, mapper, null);
    }

    /** Fail startup when the checked-in Postgres bootstrap omitted the claim table. */
    @PostConstruct
    void verifyProcessingSchema() {
        String table = jdbc.queryForObject(
            "SELECT to_regclass('public.fulfillment_processed_events')",
            String.class
        );
        if (table == null) {
            throw new IllegalStateException(
                "deploy/postgres/migrate.sh must create fulfillment_processed_events before fulfillment starts"
            );
        }
        Long leaseColumns = jdbc.queryForObject("""
            SELECT COUNT(*)
              FROM information_schema.columns
             WHERE table_schema = 'public'
               AND table_name = 'fulfillment_processed_events'
               AND column_name IN ('lease_token', 'lease_until')
            """, Long.class);
        if (leaseColumns == null || leaseColumns != 2) {
            throw new IllegalStateException(
                "deploy/postgres/migrate.sh must add fulfillment claim lease columns before fulfillment starts"
            );
        }
        Long terminalColumns = jdbc.queryForObject("""
            SELECT COUNT(*)
              FROM information_schema.columns
             WHERE table_schema = 'public'
               AND table_name = 'fulfillment_processed_events'
               AND column_name = 'dead_lettered_at'
            """, Long.class);
        if (terminalColumns == null || terminalColumns != 1) {
            throw new IllegalStateException(
                "deploy/postgres/migrate.sh must add fulfillment terminal retry state before fulfillment starts"
            );
        }
        String effectsTable = jdbc.queryForObject(
            "SELECT to_regclass('public.fulfillment_effects')",
            String.class
        );
        if (effectsTable == null) {
            throw new IllegalStateException(
                "deploy/postgres/migrate.sh must create fulfillment_effects before fulfillment starts"
            );
        }
        Long effectColumns = jdbc.queryForObject("""
            SELECT COUNT(*)
              FROM information_schema.columns
             WHERE table_schema = 'public'
               AND table_name = 'fulfillment_effects'
               AND column_name IN ('operation', 'aggregate_version', 'claim_token', 'claim_until')
            """, Long.class);
        if (effectColumns == null || effectColumns != 4) {
            throw new IllegalStateException(
                "deploy/postgres/migrate.sh must add fulfillment effect fencing columns before fulfillment starts"
            );
        }
        if (claimLeaseSeconds <= 0) {
            throw new IllegalStateException("fulfillment.claim-lease-seconds must be positive");
        }
    }

    ProcessingResult processOrder(CommerceEvent event) {
        if (!event.isFulfillmentTrigger() && !event.isCancellation()) {
            throw new PermanentEventException("unsupported order event type: " + event.eventType());
        }

        String processingKey = event.processingKey();
        ClaimDecision decision = transactions.execute(status ->
            acquireClaim(ORDER_CONSUMER, processingKey, event)
        );
        if (decision == null) {
            throw new IllegalStateException("fulfillment claim transaction returned no result");
        }
        if (decision.state() == ClaimState.COMPLETED) {
            return ProcessingResult.alreadyProcessed();
        }
        if (decision.state() == ClaimState.ACTIVE) {
            return ProcessingResult.leaseActive(decision.leaseUntil());
        }
        if (decision.state() == ClaimState.DEAD_LETTERED) {
            return ProcessingResult.terminalDeadLettered();
        }

        ProcessingResult result;
        try {
            result = transactions.execute(status -> processOrderInTransaction(event, decision));
        } catch (RuntimeException error) {
            markFailed(
                ORDER_CONSUMER,
                event.tenantId(),
                processingKey,
                decision.leaseToken(),
                error
            );
            throw error;
        }
        if (result == null) {
            throw new IllegalStateException("fulfillment transaction returned no result");
        }
        return result;
    }

    private ProcessingResult processOrderInTransaction(
        CommerceEvent event,
        ClaimDecision decision
    ) {
        OrderSnapshot order = loadOrder(event.tenantId(), event.orderId())
            .orElseThrow(() -> new RetryableEventException(
                "order is not visible in PostgreSQL yet: " + event.orderId()));
        if (!order.tenantId().equals(event.tenantId())) {
            throw new PermanentEventException("event tenant does not own order");
        }

        boolean terminalOrder = TERMINAL_ORDER_STATUSES.contains(order.status());
        if (!event.isCancellation() || event.isPaymentEvent()) {
            PaymentSnapshot payment = loadPaymentForEvent(event, order)
                .orElseThrow(() -> new RetryableEventException(
                    "payment is not visible in PostgreSQL yet: " + order.id()));
            validatePayment(event, order, payment);
        }

        if (event.isCancellation()) {
            if (NON_CANCELLABLE_ORDER_STATUSES.contains(order.status())) {
                return ProcessingResult.noOp(decision.leaseToken());
            }
            cancelOrder(order);
            CommerceEvent downstream = event.orderCancelled(mapper);
            queueEffects(event, downstream, "cancelled");
            return ProcessingResult.acquired(true, null, "cancelled", decision.leaseToken());
        }

        if (terminalOrder) {
            return ProcessingResult.noOp(decision.leaseToken());
        }

        int orderUpdated = jdbc.update("""
            UPDATE orders AS o
               SET status = CASE
                   WHEN status IN ('pending', 'confirmed', 'paid') THEN 'processing'
                   ELSE status
               END,
                   updated_at = CASE
                       WHEN status IN ('pending', 'confirmed', 'paid')
                       THEN CURRENT_TIMESTAMP
                       ELSE updated_at
                   END
             WHERE o.tenant_id = ? AND o.id = ?
               AND o.status IN ('pending', 'confirmed', 'paid', 'processing')
               AND NOT EXISTS (
                   SELECT 1
                     FROM shipments existing
                    WHERE existing.tenant_id = o.tenant_id
                      AND existing.order_id = o.id
                      AND existing.shipment_number = 1
                      AND existing.status IN ('in_transit', 'delivered', 'cancelled', 'returned')
               )
            """, order.tenantId(), order.id());
        if (orderUpdated != 1) {
            return ProcessingResult.noOp(decision.leaseToken());
        }

        String proposedShipmentId = "shipment-" + order.id() + "-1";
        int inserted = jdbc.update("""
            INSERT INTO shipments (
                id, tenant_id, order_id, shipment_number, location_id,
                carrier, service_level, tracking_number, status,
                shipping_address, shipped_at, delivered_at, created_at, updated_at
            )
            SELECT ?, o.tenant_id, o.id, 1, location.id,
                   'parcel-post', 'ground', NULL, 'label_created',
                   o.shipping_address, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
              FROM orders o
              JOIN LATERAL (
                  SELECT il.id
                    FROM inventory_locations il
                   WHERE il.tenant_id = o.tenant_id AND il.is_active
                   ORDER BY il.id
                   LIMIT 1
              ) location ON TRUE
             WHERE o.tenant_id = ? AND o.id = ?
               AND o.status IN ('pending', 'confirmed', 'paid', 'processing')
            ON CONFLICT (tenant_id, order_id, shipment_number)
            DO UPDATE SET updated_at = CURRENT_TIMESTAMP
            """, proposedShipmentId, order.tenantId(), order.id());
        if (inserted != 1) {
            throw new RetryableEventException("no active inventory location for order " + order.id());
        }

        // A seeded shipment may already own this tenant/order/number under a
        // different stable ID. Use the durable row selected by the unique
        // business key for every child write and downstream event.
        String shipmentId = jdbc.queryForObject("""
            SELECT id
              FROM shipments
             WHERE tenant_id = ? AND order_id = ? AND shipment_number = 1
            """, String.class, order.tenantId(), order.id());

        jdbc.update("""
            INSERT INTO shipment_items (id, tenant_id, shipment_id, order_item_id, quantity)
            SELECT 'shipment-item-' || item.id, item.tenant_id, ?, item.id, item.quantity
              FROM order_items item
             WHERE item.tenant_id = ? AND item.order_id = ?
            ON CONFLICT (tenant_id, shipment_id, order_item_id)
            DO UPDATE SET quantity = EXCLUDED.quantity
            """, shipmentId, order.tenantId(), order.id());

        String shipmentStatus = jdbc.queryForObject(
            "SELECT status FROM shipments WHERE tenant_id = ? AND id = ?",
            String.class,
            order.tenantId(),
            shipmentId
        );
        if (TERMINAL_SHIPMENT_STATUSES.contains(shipmentStatus)) {
            return ProcessingResult.noOp(decision.leaseToken());
        }
        CommerceEvent downstream = event.shipmentCreated(mapper, shipmentId, shipmentStatus);
        queueEffects(event, downstream, shipmentStatus);
        return ProcessingResult.acquired(false, shipmentId, shipmentStatus, decision.leaseToken());
    }

    Optional<Map<String, Object>> verifyOrder(String tenantId, String orderId) {
        List<Map<String, Object>> rows = jdbc.queryForList("""
            SELECT o.tenant_id AS tenant_id,
                   o.id AS order_id,
                   o.status AS order_status,
                   shipment.id AS shipment_id,
                   shipment.status AS shipment_status,
                   COALESCE(claim.status, 'missing') AS fulfillment_status,
                   COALESCE(notification_count.deliveries, 0) AS notification_deliveries,
                   COALESCE(notification.status, 'missing') AS notification_status,
                   COALESCE(notification.channel, 'missing') AS notification_channel,
                   COALESCE(notification.provider, 'missing') AS notification_provider,
                   COALESCE(notification.acknowledgement_reference, 'missing')
                       AS notification_acknowledgement
              FROM orders o
              LEFT JOIN LATERAL (
                  SELECT s.id, s.status
                    FROM shipments s
                   WHERE s.tenant_id = o.tenant_id AND s.order_id = o.id
                   ORDER BY s.shipment_number
                   LIMIT 1
              ) shipment ON TRUE
              LEFT JOIN LATERAL (
                  SELECT f.status
                    FROM fulfillment_processed_events f
                   WHERE f.tenant_id = o.tenant_id
                     AND f.consumer_name = ?
                     AND f.order_id = o.id
                   ORDER BY CASE f.status
                       WHEN 'completed' THEN 0
                       WHEN 'processing' THEN 1
                       WHEN 'failed' THEN 2
                       ELSE 3
                   END,
                   f.claimed_at DESC
                   LIMIT 1
              ) claim ON TRUE
              LEFT JOIN LATERAL (
                  SELECT COUNT(*) AS deliveries
                    FROM notification_deliveries n
                   WHERE n.tenant_id = o.tenant_id
                     AND n.order_id = o.id
                     AND n.status = 'delivered'
              ) notification_count ON TRUE
              LEFT JOIN LATERAL (
                  SELECT n.status,
                         messages.channel,
                         CASE messages.channel
                             WHEN 'in_app' THEN 'in_app_durable_sink'
                             ELSE 'configured_external_provider'
                         END AS provider,
                         messages.acknowledgement_reference
                    FROM notification_deliveries n
                    JOIN notification_channel_messages messages
                      ON messages.tenant_id = n.tenant_id
                     AND messages.delivery_id = n.id
                   WHERE n.tenant_id = o.tenant_id
                     AND n.order_id = o.id
                     AND n.status = 'delivered'
                   ORDER BY n.updated_at DESC, n.created_at DESC, n.id DESC
                   LIMIT 1
              ) notification ON TRUE
             WHERE o.tenant_id = ? AND o.id = ?
            """,
            ORDER_CONSUMER,
            tenantId,
            orderId
        );
        return rows.stream().findFirst();
    }

    private void validatePayment(
        CommerceEvent event,
        OrderSnapshot order,
        PaymentSnapshot payment
    ) {
        if (!event.tenantId().equals(payment.tenantId())
            || (event.paymentId() != null && !event.paymentId().equals(payment.id()))
            || !event.orderId().equals(payment.orderId())
            || !order.id().equals(payment.orderId())) {
            throw new PermanentEventException("payment does not belong to the event tenant and order");
        }
        if (payment.amountMinor() != order.totalMinor() || !payment.currency().equals(order.currency())) {
            throw new PermanentEventException("payment does not match order total or currency");
        }
        if (!event.isPaymentEvent()
            && !payment.status().equals("authorized")
            && !payment.status().equals("captured")) {
            throw new RetryableEventException(
                "payment is not settled: " + payment.status() + " for " + order.id());
        }
        if (event.isPaymentEvent() && event.totalMinor() != order.totalMinor()) {
            throw new PermanentEventException("payment event amount does not match durable order total");
        }
        if (!event.isPaymentEvent() && event.totalMinor() > 0
            && event.totalMinor() != order.totalMinor()) {
            throw new PermanentEventException("event total does not match durable order total");
        }
        if (event.isPaymentEvent() && !event.currency().equals(order.currency())) {
            throw new PermanentEventException("payment event currency does not match durable order currency");
        }
        if (!event.isPaymentEvent() && !event.currency().isBlank()
            && !event.currency().equals(order.currency())) {
            throw new PermanentEventException("event currency does not match durable order currency");
        }
        if (event.isPaymentEvent()) {
            String requiredStatus = switch (event.eventType()) {
                case "payment.authorized" -> "authorized";
                case "payment.captured", "payment.paid" -> "captured";
                case "payment.failed" -> "failed";
                default -> throw new PermanentEventException(
                    "unsupported payment event type: " + event.eventType());
            };
            if (!requiredStatus.equals(payment.status())) {
                throw new RetryableEventException(
                    "payment state has not committed for " + event.paymentId()
                );
            }
            boolean eventStatusMatches = event.eventType().equals("payment.paid")
                ? event.paymentStatus().equals("paid") || event.paymentStatus().equals("captured")
                : event.paymentStatus().equals(requiredStatus);
            if (!eventStatusMatches) {
                throw new PermanentEventException("payment event status does not match event type");
            }
        } else if (!event.paymentStatus().isBlank()
            && !event.paymentStatus().equals(payment.status())) {
            throw new RetryableEventException("event payment state has not committed yet: " + order.id());
        }
    }

    private void cancelOrder(OrderSnapshot order) {
        jdbc.update("""
            UPDATE orders
               SET status = 'cancelled',
                   updated_at = CURRENT_TIMESTAMP
             WHERE tenant_id = ? AND id = ?
               AND status IN ('pending', 'confirmed', 'paid', 'processing')
            """, order.tenantId(), order.id());
        jdbc.update("""
            UPDATE fulfillment_effects
               SET status = 'cancelled',
                   cancelled_at = CURRENT_TIMESTAMP,
                   claim_token = NULL,
                   claim_until = NULL,
                   last_error = COALESCE(last_error, 'superseded by order cancellation')
             WHERE tenant_id = ? AND order_id = ?
               AND operation = 'fulfillment'
               AND status IN ('queued', 'publishing', 'failed')
            """, order.tenantId(), order.id());
        jdbc.update("""
            UPDATE shipments
               SET status = 'cancelled',
                   updated_at = CURRENT_TIMESTAMP
             WHERE tenant_id = ? AND order_id = ?
               AND status IN ('pending', 'label_created')
            """, order.tenantId(), order.id());
    }

    private void queueEffects(
        CommerceEvent source,
        CommerceEvent downstream,
        String notificationStatus
    ) {
        String operation = source.isCancellation() ? "cancellation" : "fulfillment";
        queueEffect(
            source.tenantId(),
            source.orderId(),
            operation,
            downstream.eventKey(),
            "event",
            downstream.toJson(mapper),
            null
        );
        queueEffect(
            source.tenantId(),
            source.orderId(),
            operation,
            source.notificationEffectKey(notificationStatus),
            "notification",
            source.toJson(mapper),
            notificationStatus
        );
    }

    private void queueEffect(
        String tenantId,
        String orderId,
        String operation,
        String effectKey,
        String effectKind,
        String payload,
        String notificationStatus
    ) {
        jdbc.update("""
            INSERT INTO fulfillment_effects (
                tenant_id, order_id, effect_key, operation, effect_kind, payload,
                notification_status, aggregate_version, status, attempts,
                claim_token, claim_until, published_at, cancelled_at, last_error
            )
            SELECT ?, ?, ?, ?, ?, ?::jsonb, ?, order_row.updated_at, 'queued', 0,
                   NULL, NULL, NULL, NULL, NULL
              FROM orders order_row
             WHERE order_row.tenant_id = ? AND order_row.id = ?
            ON CONFLICT (tenant_id, effect_key) DO NOTHING
            """,
            tenantId,
            orderId,
            effectKey,
            operation,
            effectKind,
            payload,
            notificationStatus,
            tenantId,
            orderId
        );
    }

    List<FulfillmentEffect> claimEffects(CommerceEvent event, String leaseToken) {
        String operation = event.isCancellation() ? "cancellation" : "fulfillment";
        String claimToken = UUID.randomUUID().toString();
        List<FulfillmentEffect> effects = transactions.execute(status -> jdbc.query("""
            UPDATE fulfillment_effects effect
               SET status = 'publishing',
                   attempts = effect.attempts + 1,
                   claim_token = ?,
                   claim_until = CURRENT_TIMESTAMP + (? * INTERVAL '1 second')
              FROM orders order_row
             WHERE effect.tenant_id = ?
               AND effect.order_id = ?
               AND effect.operation = ?
               AND effect.status IN ('queued', 'publishing')
               AND effect.attempts < ?
               AND (effect.status = 'queued'
                    OR effect.claim_until <= CURRENT_TIMESTAMP)
               AND order_row.tenant_id = effect.tenant_id
               AND order_row.id = effect.order_id
               AND order_row.updated_at = effect.aggregate_version
               AND (
                   (? = 'cancellation' AND order_row.status = 'cancelled')
                   OR (? = 'fulfillment'
                       AND order_row.status IN ('pending', 'confirmed', 'paid', 'processing'))
               )
               AND EXISTS (
                   SELECT 1
                     FROM fulfillment_processed_events claim
                    WHERE claim.tenant_id = effect.tenant_id
                      AND claim.consumer_name = ?
                      AND claim.event_key = ?
                      AND claim.status = 'processing'
                      AND claim.lease_token = ?
                      AND claim.lease_until > CURRENT_TIMESTAMP
               )
             RETURNING effect.tenant_id, effect.order_id, effect.effect_key,
                       effect.operation, effect.effect_kind, effect.payload::TEXT AS payload,
                       effect.notification_status, effect.claim_token
            """,
            (resultSet, rowNumber) -> new FulfillmentEffect(
                resultSet.getString("tenant_id"),
                resultSet.getString("order_id"),
                resultSet.getString("effect_key"),
                resultSet.getString("operation"),
                resultSet.getString("effect_kind"),
                resultSet.getString("payload"),
                resultSet.getString("notification_status"),
                resultSet.getString("claim_token")
            ),
            claimToken,
            claimLeaseSeconds,
            event.tenantId(),
            event.orderId(),
            operation,
            MAX_CLAIM_ATTEMPTS,
            operation,
            operation,
            ORDER_CONSUMER,
            event.processingKey(),
            leaseToken
        ));
        if (effects == null) {
            throw new IllegalStateException("fulfillment effect claim transaction returned no result");
        }
        return effects;
    }

    /**
     * Validate an effect claim in a short transaction before external dispatch.
     *
     * The parent and effect locks are released when this method returns. The
     * dispatcher must run only after that point; cancellation may supersede
     * the effect meanwhile, so publishEffect fences the final state transition
     * again instead of relying on this validation snapshot.
     */
    void validateEffectClaim(
        FulfillmentEffect effect,
        String eventKey,
        String leaseToken
    ) {
        Boolean valid = transactions.execute(status -> {
            List<Boolean> locked = jdbc.query("""
                WITH locked_order AS MATERIALIZED (
                    SELECT order_row.tenant_id, order_row.id,
                           order_row.updated_at, order_row.status
                      FROM orders order_row
                     WHERE order_row.tenant_id = ?
                       AND order_row.id = ?
                     FOR UPDATE
                )
                SELECT TRUE
                  FROM fulfillment_effects effect
                  JOIN locked_order order_row
                    ON order_row.tenant_id = effect.tenant_id
                   AND order_row.id = effect.order_id
                 WHERE effect.tenant_id = order_row.tenant_id
                   AND effect.order_id = order_row.id
                   AND effect.effect_key = ?
                   AND effect.status = 'publishing'
                   AND effect.claim_token = ?
                   AND effect.claim_until > CURRENT_TIMESTAMP
                   AND effect.aggregate_version = order_row.updated_at
                   AND (
                       (effect.operation = 'cancellation' AND order_row.status = 'cancelled')
                       OR (effect.operation = 'fulfillment'
                           AND order_row.status IN ('pending', 'confirmed', 'paid', 'processing'))
                   )
                   AND EXISTS (
                       SELECT 1
                         FROM fulfillment_processed_events claim
                        WHERE claim.tenant_id = effect.tenant_id
                          AND claim.consumer_name = ?
                          AND claim.event_key = ?
                          AND claim.status = 'processing'
                          AND claim.lease_token = ?
                          AND claim.lease_until > CURRENT_TIMESTAMP
                   )
                 FOR UPDATE OF effect
                """,
                (resultSet, rowNumber) -> resultSet.getBoolean(1),
                effect.tenantId(),
                effect.orderId(),
                effect.effectKey(),
                effect.claimToken(),
                ORDER_CONSUMER,
                eventKey,
                leaseToken
            );
            if (locked.isEmpty()) {
                throw new RetryableEventException(
                    "fulfillment effect was superseded or lost its lease before dispatch for "
                        + effect.effectKey()
                );
            }
            return true;
        });
        if (!Boolean.TRUE.equals(valid)) {
            throw new IllegalStateException("fulfillment effect validation transaction returned no result");
        }
    }

    /**
     * Finalize an effect after its external dispatch has completed.
     *
     * Every predicate is repeated here because the claim, order aggregate, or
     * cancellation state can change after validateEffectClaim returns. A lost
     * fence never turns a stale external dispatch into a published durable
     * effect; the caller retries through the idempotent downstream boundary.
     */
    void publishEffect(
        FulfillmentEffect effect,
        String eventKey,
        String leaseToken
    ) {
        Boolean published = transactions.execute(status -> {
            int updated = jdbc.update("""
                UPDATE fulfillment_effects effect
                   SET status = 'published',
                       published_at = CURRENT_TIMESTAMP,
                       claim_token = NULL,
                       claim_until = NULL,
                       last_error = NULL
                  FROM orders order_row
                 WHERE effect.tenant_id = ?
                   AND effect.order_id = ?
                   AND effect.effect_key = ?
                   AND effect.status = 'publishing'
                   AND effect.claim_token = ?
                   AND effect.claim_until > CURRENT_TIMESTAMP
                   AND effect.aggregate_version = order_row.updated_at
                   AND order_row.tenant_id = effect.tenant_id
                   AND order_row.id = effect.order_id
                   AND (
                       (effect.operation = 'cancellation' AND order_row.status = 'cancelled')
                       OR (effect.operation = 'fulfillment'
                           AND order_row.status IN ('pending', 'confirmed', 'paid', 'processing'))
                   )
                   AND EXISTS (
                       SELECT 1
                         FROM fulfillment_processed_events claim
                        WHERE claim.tenant_id = effect.tenant_id
                          AND claim.consumer_name = ?
                          AND claim.event_key = ?
                          AND claim.status = 'processing'
                          AND claim.lease_token = ?
                          AND claim.lease_until > CURRENT_TIMESTAMP
                   )
                """,
                effect.tenantId(),
                effect.orderId(),
                effect.effectKey(),
                effect.claimToken(),
                ORDER_CONSUMER,
                eventKey,
                leaseToken
            );
            if (updated != 1) {
                throw new RetryableEventException(
                    "fulfillment effect lease or aggregate fence was lost before completion for "
                        + effect.effectKey()
                );
            }
            return true;
        });
        if (!Boolean.TRUE.equals(published)) {
            throw new IllegalStateException("fulfillment effect publication transaction returned no result");
        }
    }

    void releaseEffect(FulfillmentEffect effect, Throwable error) {
        String message = error.getMessage() == null
            ? error.getClass().getSimpleName()
            : error.getMessage();
        jdbc.update("""
            UPDATE fulfillment_effects
               SET status = CASE WHEN attempts >= ? THEN 'failed' ELSE 'queued' END,
                   claim_token = NULL,
                   claim_until = NULL,
                   last_error = ?
             WHERE tenant_id = ?
               AND effect_key = ?
               AND status = 'publishing'
               AND claim_token = ?
            """,
            MAX_CLAIM_ATTEMPTS,
            message.substring(0, Math.min(message.length(), 1_000)),
            effect.tenantId(),
            effect.effectKey(),
            effect.claimToken()
        );
    }

    ClaimDecision acquireClaim(String consumer, CommerceEvent event) {
        String claimKey = consumer.equals(ORDER_CONSUMER)
            ? event.processingKey()
            : event.eventKey();
        return acquireClaim(consumer, claimKey, event);
    }

    private ClaimDecision acquireClaim(String consumer, String claimKey, CommerceEvent event) {
        String leaseToken = UUID.randomUUID().toString();
        List<ClaimRow> claimed = jdbc.query("""
            WITH exhausted AS (
                UPDATE fulfillment_processed_events
                   SET status = 'dead_lettered',
                       dead_lettered_at = COALESCE(dead_lettered_at, CURRENT_TIMESTAMP),
                       last_error = COALESCE(last_error, 'fulfillment retry limit exhausted'),
                       lease_token = NULL,
                       lease_until = NULL
                 WHERE tenant_id = ?
                   AND consumer_name = ?
                   AND event_key = ?
                   AND attempts >= ?
                   AND (
                       status = 'failed'
                       OR (
                           status = 'processing'
                           AND (
                               lease_until IS NULL
                               OR lease_until <= CURRENT_TIMESTAMP
                           )
                       )
                   )
                RETURNING event_key
            )
            INSERT INTO fulfillment_processed_events (
                tenant_id, consumer_name, event_key, event_type, order_id,
                status, attempts, claimed_at, completed_at, last_error,
                lease_token, lease_until
            )
            SELECT ?, ?, ?, ?, ?, 'processing', 1, CURRENT_TIMESTAMP, NULL, NULL,
                   ?, CURRENT_TIMESTAMP + (? * INTERVAL '1 second')
             WHERE NOT EXISTS (SELECT 1 FROM exhausted)
            ON CONFLICT (tenant_id, consumer_name, event_key)
            DO UPDATE SET
                status = 'processing',
                attempts = fulfillment_processed_events.attempts + 1,
                claimed_at = CURRENT_TIMESTAMP,
                completed_at = NULL,
                last_error = NULL,
                dead_lettered_at = NULL,
                lease_token = EXCLUDED.lease_token,
                lease_until = EXCLUDED.lease_until
            WHERE (
                fulfillment_processed_events.status = 'failed'
                OR (
                    fulfillment_processed_events.status = 'processing'
                    AND (
                        fulfillment_processed_events.lease_until IS NULL
                        OR fulfillment_processed_events.lease_until <= CURRENT_TIMESTAMP
                    )
                )
            )
            AND fulfillment_processed_events.attempts < ?
            RETURNING status, attempts, lease_token, lease_until
            """,
            (resultSet, rowNumber) -> new ClaimRow(
                resultSet.getString("status"),
                resultSet.getInt("attempts"),
                resultSet.getString("lease_token"),
                resultSet.getTimestamp("lease_until").toInstant()
            ),
            event.tenantId(),
            consumer,
            claimKey,
            MAX_CLAIM_ATTEMPTS,
            event.tenantId(),
            consumer,
            claimKey,
            event.eventType(),
            event.orderId(),
            leaseToken,
            claimLeaseSeconds,
            MAX_CLAIM_ATTEMPTS
        );
        if (!claimed.isEmpty()) {
            ClaimRow row = claimed.getFirst();
            return ClaimDecision.acquired(row.leaseToken(), row.leaseUntil());
        }

        List<ClaimRow> current = jdbc.query("""
            SELECT status, attempts, lease_token, lease_until
              FROM fulfillment_processed_events
             WHERE tenant_id = ?
               AND consumer_name = ?
               AND event_key = ?
            FOR UPDATE
            """,
            (resultSet, rowNumber) -> new ClaimRow(
                resultSet.getString("status"),
                resultSet.getInt("attempts"),
                resultSet.getString("lease_token"),
                resultSet.getTimestamp("lease_until") == null
                    ? null
                    : resultSet.getTimestamp("lease_until").toInstant()
            ),
            event.tenantId(),
            consumer,
            claimKey
        );
        if (current.isEmpty()) {
            throw new RetryableEventException(
                "claim disappeared while acquiring event " + claimKey);
        }
        ClaimRow row = current.getFirst();
        return switch (row.status()) {
            case "completed" -> ClaimDecision.completed();
            case "processing" -> ClaimDecision.active(row.leaseUntil());
            case "dead_lettered" -> ClaimDecision.deadLettered();
            case "failed" -> {
                if (row.attempts() >= MAX_CLAIM_ATTEMPTS) {
                    yield ClaimDecision.deadLettered();
                }
                throw new RetryableEventException(
                    "claim is temporarily unavailable for " + claimKey);
            }
            default -> throw new RetryableEventException(
                "claim is in an unknown state " + row.status() + " for " + claimKey);
        };
    }

    void renewLease(String tenantId, String consumer, String eventKey, String leaseToken) {
        int renewed = jdbc.update("""
            UPDATE fulfillment_processed_events
               SET lease_until = CURRENT_TIMESTAMP + (? * INTERVAL '1 second')
             WHERE tenant_id = ?
               AND consumer_name = ?
               AND event_key = ?
               AND status = 'processing'
               AND lease_token = ?
               AND lease_until > CURRENT_TIMESTAMP
            """, claimLeaseSeconds, tenantId, consumer, eventKey, leaseToken);
        if (renewed != 1) {
            throw new RetryableEventException(
                "fulfillment claim lease is no longer owned for " + eventKey);
        }
    }

    LeaseHeartbeat maintainLease(
        String tenantId,
        String consumer,
        String eventKey,
        String leaseToken
    ) {
        if (leaseExecutor == null) {
            throw new IllegalStateException(
                "fulfillment claim lease heartbeat executor is not configured"
            );
        }
        long leaseMillis = TimeUnit.SECONDS.toMillis(claimLeaseSeconds);
        long intervalMillis = Math.max(100L, Math.min(leaseMillis / 3L, 10_000L));
        AtomicReference<RuntimeException> failure = new AtomicReference<>();
        AtomicReference<ScheduledFuture<?>> scheduled = new AtomicReference<>();
        Runnable renewal = () -> {
            if (failure.get() != null) {
                return;
            }
            try {
                renewLease(tenantId, consumer, eventKey, leaseToken);
            } catch (RuntimeException error) {
                failure.compareAndSet(null, error);
                ScheduledFuture<?> task = scheduled.get();
                if (task != null) {
                    task.cancel(false);
                }
            }
        };
        ScheduledFuture<?> future = leaseExecutor.scheduleAtFixedRate(
            renewal,
            intervalMillis,
            intervalMillis,
            TimeUnit.MILLISECONDS
        );
        scheduled.set(future);
        if (failure.get() != null) {
            future.cancel(false);
        }
        return new LeaseHeartbeat(future, failure, eventKey);
    }

    void markCompleted(String tenantId, String consumer, String eventKey, String leaseToken) {
        int completed = jdbc.update("""
            UPDATE fulfillment_processed_events
               SET status = 'completed',
                   completed_at = CURRENT_TIMESTAMP,
                   last_error = NULL,
                   lease_token = NULL,
                   lease_until = NULL
             WHERE tenant_id = ?
               AND consumer_name = ?
               AND event_key = ?
               AND status = 'processing'
               AND lease_token = ?
               AND lease_until > CURRENT_TIMESTAMP
            """, tenantId, consumer, eventKey, leaseToken);
        if (completed != 1) {
            throw new RetryableEventException(
                "fulfillment claim lease was lost before completion for " + eventKey);
        }
    }

    void markFailed(
        String consumer,
        String tenantId,
        String eventKey,
        String leaseToken,
        Throwable error
    ) {
        boolean permanentFailure = error instanceof PermanentEventException;
        String message = error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage();
        int failed = jdbc.update("""
               UPDATE fulfillment_processed_events
               SET status = CASE
                   WHEN ? THEN 'dead_lettered'
                   WHEN attempts >= ? THEN 'dead_lettered'
                   ELSE 'failed'
               END,
                   last_error = ?,
                   claimed_at = CURRENT_TIMESTAMP,
                   dead_lettered_at = CASE
                       WHEN ? THEN CURRENT_TIMESTAMP
                       WHEN attempts >= ? THEN CURRENT_TIMESTAMP
                       ELSE NULL
                   END,
                   lease_token = NULL,
                   lease_until = NULL
             WHERE tenant_id = ?
               AND consumer_name = ?
               AND event_key = ?
               AND status = 'processing'
               AND lease_token = ?
               AND (? OR lease_until > CURRENT_TIMESTAMP)
            """,
            permanentFailure,
            MAX_CLAIM_ATTEMPTS,
            message.substring(0, Math.min(message.length(), 1_000)),
            permanentFailure,
            MAX_CLAIM_ATTEMPTS,
            tenantId,
            consumer,
            eventKey,
            leaseToken,
            permanentFailure
        );
        if (failed != 1) {
            throw new RetryableEventException(
                "fulfillment claim lease was lost before failure transition for " + eventKey
            );
        }
    }

    AnalyticsClaim claimAnalytics(CommerceEvent event) {
        ClaimDecision decision = transactions.execute(status ->
            acquireClaim(ANALYTICS_CONSUMER, event)
        );
        if (decision == null) {
            throw new IllegalStateException("analytics claim transaction returned no result");
        }
        if (decision.state() == ClaimState.COMPLETED) {
            return AnalyticsClaim.alreadyProcessed();
        }
        if (decision.state() == ClaimState.ACTIVE) {
            return AnalyticsClaim.activeLease();
        }
        if (decision.state() == ClaimState.DEAD_LETTERED) {
            return AnalyticsClaim.terminalDeadLettered();
        }

        try {
            AnalyticsClaim claim = transactions.execute(status -> {
            var spanContext = Span.current().getSpanContext();
            String traceId = spanContext.isValid() ? spanContext.getTraceId() : "";
            String spanId = spanContext.isValid() ? spanContext.getSpanId() : "";
            String analyticsIdentity = event.tenantId() + "\u0000" + event.eventKey();
            String analyticsId = "analytics-" + UUID.nameUUIDFromBytes(
                analyticsIdentity.getBytes(StandardCharsets.UTF_8));
            jdbc.update("""
                INSERT INTO analytics_events (
                    id, tenant_id, event_key, customer_id, session_id, event_name,
                    event_version, source, entity_type, entity_id, occurred_at,
                    trace_id, span_id, properties, context
                ) VALUES (?, ?, ?, ?, ?, ?, ?, 'fulfillment', ?, ?, ?, ?, ?, ?::jsonb, ?::jsonb)
                ON CONFLICT (tenant_id, event_key) DO NOTHING
                """,
                analyticsId,
                event.tenantId(),
                event.eventKey(),
                blankAsNull(event.customerId()),
                blankAsNull(event.sessionId()),
                event.eventType(),
                event.schemaVersion(),
                event.entityType(),
                event.entityId(),
                Timestamp.from(event.occurredAt()),
                traceId,
                spanId,
                event.payload().toString(),
                event.context().toString()
            );
                return AnalyticsClaim.acquired(analyticsId, traceId, spanId, decision.leaseToken());
            });
            if (claim == null) {
                throw new IllegalStateException("analytics event transaction returned no result");
            }
            return claim;
        } catch (RuntimeException error) {
            try {
                markFailed(
                    ANALYTICS_CONSUMER,
                    event.tenantId(),
                    event.eventKey(),
                    decision.leaseToken(),
                    error
                );
            } catch (RuntimeException markFailedError) {
                error.addSuppressed(markFailedError);
            }
            throw error;
        }
    }

    Optional<CommerceEvent> loadOrderEvent(String tenantId, String orderId) {
        List<OrderSnapshot> orders = jdbc.query("""
            SELECT o.id, o.tenant_id, o.customer_id, o.currency, o.total_minor,
                   o.status, COALESCE(payment.status, 'pending') AS payment_status,
                   COALESCE(o.placed_at, o.created_at) AS occurred_at
              FROM orders o
              LEFT JOIN LATERAL (
                  SELECT p.status
                    FROM payments p
                   WHERE p.tenant_id = o.tenant_id AND p.order_id = o.id
                   ORDER BY p.created_at DESC
                   LIMIT 1
              ) payment ON TRUE
             WHERE o.tenant_id = ? AND o.id = ?
            """,
            (resultSet, rowNumber) -> new OrderSnapshot(
                resultSet.getString("id"),
                resultSet.getString("tenant_id"),
                resultSet.getString("customer_id"),
                resultSet.getString("currency"),
                resultSet.getLong("total_minor"),
                resultSet.getString("status"),
                resultSet.getString("payment_status"),
                resultSet.getTimestamp("occurred_at").toInstant()
            ),
            tenantId,
            orderId
        );
        if (orders.isEmpty()) {
            return Optional.empty();
        }
        OrderSnapshot order = orders.getFirst();
        ObjectNode payload = mapper.createObjectNode();
        payload.put("order_id", order.id());
        payload.put("tenant_id", order.tenantId());
        if (order.customerId() != null) {
            payload.put("customer_id", order.customerId());
        }
        payload.put("currency", order.currency());
        payload.put("total_minor", order.totalMinor());
        payload.put("status", order.status());
        payload.put("payment_status", order.paymentStatus());
        ArrayNode items = payload.putArray("items");
        jdbc.query("""
            SELECT sku, product_name, quantity, unit_price_minor, discount_minor
              FROM order_items
             WHERE tenant_id = ? AND order_id = ?
             ORDER BY id
            """,
            (resultSet, rowNumber) -> {
                ObjectNode item = mapper.createObjectNode();
                item.put("sku", resultSet.getString("sku"));
                item.put("product_name", resultSet.getString("product_name"));
                item.put("quantity", resultSet.getInt("quantity"));
                item.put("unit_price_minor", resultSet.getLong("unit_price_minor"));
                item.put("discount_minor", resultSet.getLong("discount_minor"));
                items.add(item);
                return item;
            },
            order.tenantId(),
            order.id()
        );
        return Optional.of(new CommerceEvent(
            "order-event-" + order.id(),
            "order.created:" + order.id(),
            1,
            order.tenantId(),
            "order.created",
            order.occurredAt(),
            order.id(),
            order.customerId(),
            order.currency(),
            order.totalMinor(),
            order.paymentStatus(),
            payload
        ));
    }

    private Optional<OrderSnapshot> loadOrder(String tenantId, String orderId) {
        List<OrderSnapshot> orders = jdbc.query("""
            SELECT o.id, o.tenant_id, o.customer_id, o.currency, o.total_minor,
                   o.status, COALESCE(o.created_at, CURRENT_TIMESTAMP) AS occurred_at,
                   COALESCE(payment.status, 'pending') AS payment_status
              FROM orders o
              LEFT JOIN LATERAL (
                  SELECT p.status
                    FROM payments p
                   WHERE p.tenant_id = o.tenant_id AND p.order_id = o.id
                   ORDER BY p.created_at DESC
                   LIMIT 1
              ) payment ON TRUE
             WHERE o.tenant_id = ? AND o.id = ?
             FOR UPDATE OF o
            """,
            (resultSet, rowNumber) -> new OrderSnapshot(
                resultSet.getString("id"),
                resultSet.getString("tenant_id"),
                resultSet.getString("customer_id"),
                resultSet.getString("currency"),
                resultSet.getLong("total_minor"),
                resultSet.getString("status"),
                resultSet.getString("payment_status"),
                resultSet.getTimestamp("occurred_at").toInstant()
            ),
            tenantId,
            orderId
        );
        return orders.stream().findFirst();
    }

    private Optional<PaymentSnapshot> loadPaymentForEvent(
        CommerceEvent event,
        OrderSnapshot order
    ) {
        String paymentId = event.paymentId();
        if (paymentId != null && !paymentId.isBlank()) {
            return loadPayment(event.tenantId(), paymentId);
        }
        return loadLatestPayment(order.tenantId(), order.id());
    }

    private Optional<PaymentSnapshot> loadPayment(String tenantId, String paymentId) {
        List<PaymentSnapshot> payments = jdbc.query("""
            SELECT id, tenant_id, order_id, status, amount_minor, currency
              FROM payments
             WHERE tenant_id = ? AND id = ?
            """,
            (resultSet, rowNumber) -> new PaymentSnapshot(
                resultSet.getString("id"),
                resultSet.getString("tenant_id"),
                resultSet.getString("order_id"),
                resultSet.getString("status"),
                resultSet.getLong("amount_minor"),
                resultSet.getString("currency")
            ),
            tenantId,
            paymentId
        );
        return payments.stream().findFirst();
    }

    private Optional<PaymentSnapshot> loadLatestPayment(String tenantId, String orderId) {
        List<PaymentSnapshot> payments = jdbc.query("""
            SELECT id, tenant_id, order_id, status, amount_minor, currency
              FROM payments
             WHERE tenant_id = ? AND order_id = ?
             ORDER BY created_at DESC
             LIMIT 1
            """,
            (resultSet, rowNumber) -> new PaymentSnapshot(
                resultSet.getString("id"),
                resultSet.getString("tenant_id"),
                resultSet.getString("order_id"),
                resultSet.getString("status"),
                resultSet.getLong("amount_minor"),
                resultSet.getString("currency")
            ),
            tenantId,
            orderId
        );
        return payments.stream().findFirst();
    }

    private static String blankAsNull(String value) {
        return value == null || value.isBlank() ? null : value;
    }
}

final class LeaseHeartbeat implements AutoCloseable {
    private final ScheduledFuture<?> future;
    private final AtomicReference<RuntimeException> failure;
    private final String eventKey;
    private final AtomicBoolean closed;

    LeaseHeartbeat(
        ScheduledFuture<?> future,
        AtomicReference<RuntimeException> failure,
        String eventKey
    ) {
        this(future, failure, eventKey, new AtomicBoolean(false));
    }

    private LeaseHeartbeat(
        ScheduledFuture<?> future,
        AtomicReference<RuntimeException> failure,
        String eventKey,
        AtomicBoolean closed
    ) {
        this.future = future;
        this.failure = failure;
        this.eventKey = eventKey;
        this.closed = closed;
    }

    static LeaseHeartbeat inactive() {
        return new LeaseHeartbeat(null, new AtomicReference<>(), "inactive");
    }

    void verify() {
        RuntimeException error = failure.get();
        if (error != null) {
            throw new RetryableEventException(
                "fulfillment claim heartbeat lost lease for " + eventKey,
                error
            );
        }
        if (future != null && future.isDone() && !closed.get()) {
            throw new RetryableEventException(
                "fulfillment claim heartbeat stopped for " + eventKey
            );
        }
    }

    @Override
    public void close() {
        if (future != null) {
            closed.set(true);
            future.cancel(false);
        }
        verify();
    }
}

record OrderSnapshot(
    String id,
    String tenantId,
    String customerId,
    String currency,
    long totalMinor,
    String status,
    String paymentStatus,
    Instant occurredAt
) {}

record PaymentSnapshot(
    String id,
    String tenantId,
    String orderId,
    String status,
    long amountMinor,
    String currency
) {}

record FulfillmentEffect(
    String tenantId,
    String orderId,
    String effectKey,
    String operation,
    String effectKind,
    String payload,
    String notificationStatus,
    String claimToken
) {}

enum ClaimState {
    ACQUIRED,
    ACTIVE,
    COMPLETED,
    DEAD_LETTERED
}

record ClaimRow(String status, int attempts, String leaseToken, Instant leaseUntil) {}

record ClaimDecision(ClaimState state, String leaseToken, Instant leaseUntil) {
    static ClaimDecision acquired(String leaseToken, Instant leaseUntil) {
        return new ClaimDecision(ClaimState.ACQUIRED, leaseToken, leaseUntil);
    }

    static ClaimDecision active(Instant leaseUntil) {
        return new ClaimDecision(ClaimState.ACTIVE, null, leaseUntil);
    }

    static ClaimDecision completed() {
        return new ClaimDecision(ClaimState.COMPLETED, null, null);
    }

    static ClaimDecision deadLettered() {
        return new ClaimDecision(ClaimState.DEAD_LETTERED, null, null);
    }
}

record ProcessingResult(
    ClaimState state,
    boolean cancellation,
    String shipmentId,
    String shipmentStatus,
    String leaseToken,
    Instant leaseUntil,
    boolean noOp
) {
    static ProcessingResult acquired(
        boolean cancellation,
        String shipmentId,
        String shipmentStatus,
        String leaseToken
    ) {
        return new ProcessingResult(
            ClaimState.ACQUIRED,
            cancellation,
            shipmentId,
            shipmentStatus,
            leaseToken,
            null,
            false
        );
    }

    static ProcessingResult noOp(String leaseToken) {
        return new ProcessingResult(ClaimState.ACQUIRED, false, null, null, leaseToken, null, true);
    }

    static ProcessingResult alreadyProcessed() {
        return new ProcessingResult(ClaimState.COMPLETED, false, null, null, null, null, false);
    }

    static ProcessingResult leaseActive(Instant leaseUntil) {
        return new ProcessingResult(ClaimState.ACTIVE, false, null, null, null, leaseUntil, false);
    }

    static ProcessingResult terminalDeadLettered() {
        return new ProcessingResult(ClaimState.DEAD_LETTERED, false, null, null, null, null, false);
    }

    boolean acquired() {
        return state == ClaimState.ACQUIRED;
    }

    boolean duplicate() {
        return state == ClaimState.COMPLETED;
    }

    boolean leaseActive() {
        return state == ClaimState.ACTIVE;
    }

    boolean deadLettered() {
        return state == ClaimState.DEAD_LETTERED;
    }
}

record AnalyticsClaim(
    ClaimState state,
    String analyticsId,
    String traceId,
    String spanId,
    String leaseToken
) {
    static AnalyticsClaim acquired(
        String analyticsId,
        String traceId,
        String spanId,
        String leaseToken
    ) {
        return new AnalyticsClaim(ClaimState.ACQUIRED, analyticsId, traceId, spanId, leaseToken);
    }

    static AnalyticsClaim alreadyProcessed() {
        return new AnalyticsClaim(ClaimState.COMPLETED, null, null, null, null);
    }

    static AnalyticsClaim activeLease() {
        return new AnalyticsClaim(ClaimState.ACTIVE, null, null, null, null);
    }

    static AnalyticsClaim terminalDeadLettered() {
        return new AnalyticsClaim(ClaimState.DEAD_LETTERED, null, null, null, null);
    }

    boolean duplicate() {
        return state == ClaimState.COMPLETED;
    }

    boolean leaseActive() {
        return state == ClaimState.ACTIVE;
    }

    boolean deadLettered() {
        return state == ClaimState.DEAD_LETTERED;
    }
}
