package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.inOrder;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.rabbitmq.client.Channel;
import io.opentelemetry.api.common.AttributeKey;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.baggage.Baggage;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.SpanContext;
import io.opentelemetry.api.trace.StatusCode;
import io.opentelemetry.api.trace.TraceFlags;
import io.opentelemetry.api.trace.TraceState;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.Scope;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;
import org.mockito.Answers;
import org.mockito.MockedStatic;
import org.springframework.amqp.core.Message;
import org.springframework.amqp.core.MessageProperties;

class OrderConsumerTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void handles_a_real_paid_order_before_acknowledging_the_delivery() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-1", "captured");
        when(repository.processOrder(any())).thenReturn(
            ProcessingResult.acquired(false, "shipment-order-1-1", "label_created", "lease-1")
        );
        stubEffects(repository, event, false, "lease-1");
        stubHeartbeat(repository);

        consumer.onOrder(message(event, 42), channel);

        verify(publisher).publish(any(CommerceEvent.class), any(Context.class));
        verify(notifications).notifyOrder(eq(event), eq("label_created"), any(Context.class));
        verify(repository).markCompleted(
            event.tenantId(), FulfillmentRepository.ORDER_CONSUMER, event.processingKey(), "lease-1");
        verify(channel).basicAck(42, false);
        var effectOrder = inOrder(repository, publisher, notifications);
        effectOrder.verify(repository).validateEffectClaim(
            any(FulfillmentEffect.class),
            eq(event.processingKey()),
            eq("lease-1")
        );
        effectOrder.verify(publisher).publish(any(CommerceEvent.class), any(Context.class));
        effectOrder.verify(repository).publishEffect(
            any(FulfillmentEffect.class),
            eq(event.processingKey()),
            eq("lease-1")
        );
        effectOrder.verify(repository).validateEffectClaim(
            any(FulfillmentEffect.class),
            eq(event.processingKey()),
            eq("lease-1")
        );
        effectOrder.verify(notifications).notifyOrder(
            eq(event), eq("label_created"), any(Context.class)
        );
        effectOrder.verify(repository).publishEffect(
            any(FulfillmentEffect.class),
            eq(event.processingKey()),
            eq("lease-1")
        );
    }

    @Test
    void duplicate_database_claim_is_acked_without_repeating_side_effects() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("payment.captured", "order-2", "captured");
        when(repository.processOrder(any())).thenReturn(ProcessingResult.alreadyProcessed());

        consumer.onOrder(message(event, 43), channel);

        verify(channel).basicAck(43, false);
        verify(publisher, never()).publish(any(), any());
        verify(notifications, never()).notifyOrder(any(), any(), any());
        verify(repository, never()).markCompleted(any(), any(), any(), any());
    }

    @Test
    void rejects_a_route_body_mismatch_before_repository_side_effects() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventConsumer consumer = new OrderEventConsumer(
            MAPPER, repository, mock(OrderEventPublisher.class), mock(NotificationClient.class)
        );
        Channel channel = mock(Channel.class);
        Message message = message(event("order.created", "order-route", "captured"), 430);
        message.getMessageProperties().setReceivedRoutingKey("order.cancelled");

        consumer.onOrder(message, channel);

        verify(channel).basicReject(430, false);
        verify(repository, never()).processOrder(any());
    }

    @Test
    void rejects_a_message_without_broker_identity_before_repository_side_effects() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventConsumer consumer = new OrderEventConsumer(
            MAPPER, repository, mock(OrderEventPublisher.class), mock(NotificationClient.class)
        );
        Channel channel = mock(Channel.class);
        Message message = message(event("order.created", "order-identity-missing", "captured"), 433);
        message.getMessageProperties().getHeaders().remove("event-key");

        consumer.onOrder(message, channel);

        verify(channel).basicReject(433, false);
        verify(repository, never()).processOrder(any());
    }

    @Test
    void rejects_an_order_event_without_complete_w3c_context() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventConsumer consumer = new OrderEventConsumer(
            MAPPER, repository, mock(OrderEventPublisher.class), mock(NotificationClient.class)
        );
        Channel channel = mock(Channel.class);
        Message message = message(event("order.created", "order-context-missing", "captured"), 431);
        message.getMessageProperties().getHeaders().remove("baggage");

        consumer.onOrder(message, channel);

        verify(channel).basicReject(431, false);
        verify(repository, never()).processOrder(any());
    }

    @Test
    void rejects_an_order_event_with_invalid_w3c_context() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventConsumer consumer = new OrderEventConsumer(
            MAPPER, repository, mock(OrderEventPublisher.class), mock(NotificationClient.class)
        );
        Channel channel = mock(Channel.class);
        Message message = message(event("order.created", "order-context-invalid", "captured"), 432);
        message.getMessageProperties().setHeader("tracestate", "invalid");

        consumer.onOrder(message, channel);

        verify(channel).basicReject(432, false);
        verify(repository, never()).processOrder(any());
    }

    @Test
    void retry_route_requires_the_preserved_original_event_route() {
        MessageProperties source = new MessageProperties();
        source.setReceivedExchange(RabbitMessagingConfiguration.EVENTS_EXCHANGE);
        source.setReceivedRoutingKey("payment.captured");
        MessageProperties retry = new MessageProperties();

        RabbitDeliveryRoute.preserveOriginalRoute(source, retry);

        assertEquals(
            RabbitMessagingConfiguration.EVENTS_EXCHANGE,
            retry.getHeaders().get(RabbitDeliveryRoute.ORIGINAL_EXCHANGE_HEADER)
        );
        assertEquals(
            "payment.captured",
            retry.getHeaders().get(RabbitDeliveryRoute.ORIGINAL_ROUTING_KEY_HEADER)
        );
    }

    @Test
    void acknowledges_a_terminal_order_without_creating_downstream_effects() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.paid", "order-terminal", "captured");
        when(repository.processOrder(any())).thenReturn(
            ProcessingResult.noOp("fulfillment:tenant-acme:order-terminal")
        );
        stubHeartbeat(repository);

        consumer.onOrder(message(event, 43), channel);

        verify(publisher, never()).publish(any(), any());
        verify(notifications, never()).notifyOrder(any(), any(), any());
        verify(repository).markCompleted(
            event.tenantId(),
            FulfillmentRepository.ORDER_CONSUMER,
            event.processingKey(),
            "fulfillment:tenant-acme:order-terminal"
        );
        verify(channel).basicAck(43, false);
    }

    @Test
    void records_failed_claim_and_leaves_delivery_for_container_retry() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-3", "captured");
        RuntimeException failure = new RetryableEventException("database unavailable");
        when(repository.processOrder(any())).thenThrow(failure);

        assertThrows(RetryableEventException.class, () -> consumer.onOrder(message(event, 44), channel));

        verify(repository, never()).markFailed(any(), any(), any(), any(), any());
        verify(channel, never()).basicAck(44, false);
        verify(publisher, never()).publish(any(), any());
    }

    @Test
    void dead_letters_a_claimed_permanent_effect_before_rejecting_the_delivery() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-permanent", "captured");
        when(repository.processOrder(any())).thenReturn(
            ProcessingResult.acquired(false, "shipment-order-permanent-1", "label_created", "lease-permanent")
        );
        when(repository.claimEffects(any(), eq("lease-permanent"))).thenReturn(List.of(
            new FulfillmentEffect(
                event.tenantId(), event.orderId(), event.fulfillmentEffectKey(), "fulfillment",
                "unknown", event.toJson(MAPPER), null, "effect-permanent"
            )
        ));
        stubHeartbeat(repository);

        consumer.onOrder(message(event, 441), channel);

        verify(repository).markFailed(
            eq(FulfillmentRepository.ORDER_CONSUMER),
            eq(event.tenantId()),
            eq(event.processingKey()),
            eq("lease-permanent"),
            org.mockito.ArgumentMatchers.any(PermanentEventException.class)
        );
        verify(repository).releaseEffect(any(), any(PermanentEventException.class));
        verify(channel).basicReject(441, false);
        verify(channel, never()).basicAck(441, false);
    }

    @Test
    void does_not_dispatch_after_the_effect_claim_is_lost_during_validation() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-validation-lost", "captured");
        when(repository.processOrder(any())).thenReturn(
            ProcessingResult.acquired(
                false,
                "shipment-order-validation-lost-1",
                "label_created",
                "lease-validation-lost"
            )
        );
        stubEffects(repository, event, false, "lease-validation-lost");
        stubHeartbeat(repository);
        RetryableEventException lostClaim = new RetryableEventException(
            "effect claim was lost before dispatch"
        );
        doThrow(lostClaim).when(repository).validateEffectClaim(
            any(FulfillmentEffect.class),
            anyString(),
            anyString()
        );

        assertThrows(
            RetryableEventException.class,
            () -> consumer.onOrder(message(event, 442), channel)
        );

        verify(publisher, never()).publish(any(), any());
        verify(notifications, never()).notifyOrder(any(), any(), any());
        verify(repository).releaseEffect(any(FulfillmentEffect.class), eq(lostClaim));
        verify(channel, never()).basicAck(442, false);
    }

    @Test
    void defers_an_active_claim_without_acknowledging_work_as_complete() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-4", "captured");
        Message message = message(event, 45);
        when(repository.processOrder(any())).thenReturn(
            ProcessingResult.leaseActive(Instant.now().plusSeconds(30))
        );

        consumer.onOrder(message, channel);

        verify(publisher).defer(
            eq(message),
            eq(RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY),
            any(Context.class)
        );
        verify(channel).basicAck(45, false);
        verify(repository, never()).markCompleted(any(), any(), any(), any());
        verify(repository, never()).markFailed(any(), any(), any(), any(), any());
    }

    @Test
    void replays_downstream_work_after_a_publisher_failure() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-5", "captured");
        ProcessingResult acquired = ProcessingResult.acquired(
            false,
            "shipment-order-5-1",
            "label_created",
            "lease-5"
        );
        when(repository.processOrder(any())).thenReturn(acquired);
        stubEffects(repository, event, false, "lease-5");
        stubHeartbeat(repository);
        RuntimeException failure = new RetryableEventException("publisher unavailable");
        doThrow(failure).doNothing().when(publisher).publish(any(), any());

        assertThrows(
            RetryableEventException.class,
            () -> consumer.onOrder(message(event, 46), channel)
        );
        consumer.onOrder(message(event, 47), channel);

        verify(repository).markFailed(
            FulfillmentRepository.ORDER_CONSUMER,
            event.tenantId(),
            event.processingKey(),
            "lease-5",
            failure
        );
        verify(repository).markCompleted(
            event.tenantId(),
            FulfillmentRepository.ORDER_CONSUMER,
            event.processingKey(),
            "lease-5"
        );
        verify(publisher, org.mockito.Mockito.times(2)).publish(any(), any());
        verify(notifications).notifyOrder(eq(event), eq("label_created"), any(Context.class));
        verify(channel).basicAck(47, false);
        verify(channel, never()).basicAck(46, false);
    }

    @Test
    void consumer_uses_incoming_w3c_context_for_real_downstream_calls() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-6", "captured");
        when(repository.processOrder(any())).thenReturn(
            ProcessingResult.acquired(false, "shipment-order-6-1", "label_created", "lease-6")
        );
        stubEffects(repository, event, false, "lease-6");
        stubHeartbeat(repository);
        SpanContext parent = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.builder().put("vendor", "state").build()
        );
        Context source = Context.root()
            .with(Span.wrap(parent))
            .with(Baggage.builder().put("tenant.id", "tenant-acme").build());
        Message message = message(event, 48);
        RabbitTraceContext.inject(source, message.getMessageProperties());
        AtomicReference<Context> downstream = new AtomicReference<>();
        doAnswer(invocation -> {
            downstream.set(invocation.getArgument(1));
            return null;
        }).when(publisher).publish(any(), any());

        try (Scope ignored = source.makeCurrent()) {
            consumer.onOrder(message, channel);
        }

        SpanContext extracted = Span.fromContext(downstream.get()).getSpanContext();
        assertEquals(parent.getTraceId(), extracted.getTraceId());
        assertEquals("state", extracted.getTraceState().get("vendor"));
        assertEquals(
            "tenant-acme",
            Baggage.fromContext(downstream.get()).getEntryValue("tenant.id")
        );
    }

    @Test
    void consumer_span_links_producer_and_parents_downstream_work() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        NotificationClient notifications = mock(NotificationClient.class);
        Channel channel = mock(Channel.class);
        OrderEventConsumer consumer = new OrderEventConsumer(MAPPER, repository, publisher, notifications);
        CommerceEvent event = event("order.created", "order-7", "captured");
        when(repository.processOrder(any())).thenReturn(
            ProcessingResult.acquired(false, "shipment-order-7-1", "label_created", "lease-7")
        );
        stubEffects(repository, event, false, "lease-7");
        stubHeartbeat(repository);

        SpanContext producer = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.builder().put("vendor", "state").build()
        );
        SpanContext consumerSpanContext = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "4bf92f3577b34da6",
            TraceFlags.getSampled(),
            TraceState.builder().put("vendor", "state").build()
        );
        RecordingSpan consumerSpan = new RecordingSpan(consumerSpanContext);
        Context source = Context.root()
            .with(Span.wrap(producer))
            .with(Baggage.builder().put("tenant.id", "tenant-acme").build());
        Message message = message(event, 49);
        RabbitTraceContext.inject(source, message.getMessageProperties());
        AtomicReference<Context> downstream = new AtomicReference<>();
        doAnswer(invocation -> {
            downstream.set(invocation.getArgument(1));
            return null;
        }).when(publisher).publish(any(), any());

        try (MockedStatic<Span> spans = org.mockito.Mockito.mockStatic(
            Span.class,
            Answers.CALLS_REAL_METHODS
        )) {
            spans.when(Span::current).thenReturn(consumerSpan);
            consumer.onOrder(message, channel);
        }

        assertEquals(producer.getTraceId(), consumerSpan.linkedSpan().getTraceId());
        assertEquals(producer.getSpanId(), consumerSpan.linkedSpan().getSpanId());
        SpanContext downstreamSpan = Span.fromContext(downstream.get()).getSpanContext();
        assertEquals(consumerSpanContext.getTraceId(), downstreamSpan.getTraceId());
        assertEquals(consumerSpanContext.getSpanId(), downstreamSpan.getSpanId());
        assertNotEquals(producer.getSpanId(), downstreamSpan.getSpanId());
    }

    private static final class RecordingSpan implements Span {
        private final SpanContext context;
        private SpanContext linkedSpan;

        private RecordingSpan(SpanContext context) {
            this.context = context;
        }

        @Override
        public <T> Span setAttribute(AttributeKey<T> key, T value) {
            return this;
        }

        @Override
        public Span addEvent(String name, Attributes attributes) {
            return this;
        }

        @Override
        public Span addEvent(
            String name,
            Attributes attributes,
            long timestamp,
            TimeUnit unit
        ) {
            return this;
        }

        @Override
        public Span setStatus(StatusCode status, String description) {
            return this;
        }

        @Override
        public Span recordException(Throwable exception, Attributes attributes) {
            return this;
        }

        @Override
        public Span updateName(String name) {
            return this;
        }

        @Override
        public Span addLink(SpanContext link) {
            linkedSpan = link;
            return this;
        }

        @Override
        public Span addLink(SpanContext link, Attributes attributes) {
            linkedSpan = link;
            return this;
        }

        @Override
        public void end() {}

        @Override
        public void end(long timestamp, TimeUnit unit) {}

        @Override
        public SpanContext getSpanContext() {
            return context;
        }

        @Override
        public boolean isRecording() {
            return true;
        }

        @Override
        public Context storeInContext(Context context) {
            return Span.wrap(this.context).storeInContext(context);
        }

        private SpanContext linkedSpan() {
            return linkedSpan;
        }
    }

    private static Message message(CommerceEvent event, long deliveryTag) {
        MessageProperties properties = new MessageProperties();
        properties.setDeliveryTag(deliveryTag);
        properties.setReceivedExchange(RabbitMessagingConfiguration.EVENTS_EXCHANGE);
        properties.setReceivedRoutingKey(event.eventType());
        properties.setMessageId(event.eventId());
        properties.setHeader("event-key", event.eventKey());
        properties.setHeader("event-type", event.eventType());
        properties.setHeader("tenant-id", event.tenantId());
        properties.setHeader(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        );
        properties.setHeader("tracestate", "vendor=state");
        properties.setHeader("baggage", "tenant.id=tenant-acme");
        return new Message(event.toJson(MAPPER).getBytes(StandardCharsets.UTF_8), properties);
    }

    private static void stubHeartbeat(FulfillmentRepository repository) {
        when(repository.maintainLease(anyString(), anyString(), anyString(), anyString()))
            .thenAnswer(invocation -> LeaseHeartbeat.inactive());
    }

    private static void stubEffects(
        FulfillmentRepository repository,
        CommerceEvent source,
        boolean cancellation,
        String claimToken
    ) {
        String status = cancellation ? "cancelled" : "label_created";
        CommerceEvent downstream = cancellation
            ? source.orderCancelled(MAPPER)
            : source.shipmentCreated(MAPPER, "shipment-" + source.orderId() + "-1", status);
        String operation = cancellation ? "cancellation" : "fulfillment";
        when(repository.claimEffects(any(), eq(claimToken))).thenReturn(List.of(
            new FulfillmentEffect(
                source.tenantId(), source.orderId(), downstream.eventKey(), operation,
                "event", downstream.toJson(MAPPER), null, "effect-event"
            ),
            new FulfillmentEffect(
                source.tenantId(), source.orderId(), source.notificationEffectKey(status), operation,
                "notification", source.toJson(MAPPER), status, "effect-notification"
            )
        ));
    }

    private static CommerceEvent event(String type, String orderId, String paymentStatus) {
        String aggregateType = type.startsWith("payment.") ? "payment" : "order";
        String aggregateId = aggregateType.equals("payment")
            ? "payment-" + orderId
            : orderId;
        String paymentId = aggregateType.equals("payment")
            ? "                \"payment_id\":\"" + aggregateId + "\",\n"
            : "";
        return CommerceEvent.parse(MAPPER, ("""
            {
              "event_id":"event-%s",
              "event_key":"%s:%s",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"%s",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"%s",
              "aggregate_id":"%s",
              "payload":{
                "order_id":"%s",
%s
                "customer_id":"customer-acme-ava",
                "currency":"USD",
                "total_minor":9177,
                "payment_status":"%s"
              }
            }
            """).formatted(
                type,
                type,
                orderId,
                type,
                aggregateType,
                aggregateId,
                orderId,
                paymentId,
                paymentStatus
            )
            .getBytes(StandardCharsets.UTF_8));
    }
}
