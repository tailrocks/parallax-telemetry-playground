package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.ArgumentCaptor.forClass;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.Mockito.doAnswer;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.verifyNoInteractions;
import static org.mockito.Mockito.when;

import com.fasterxml.jackson.databind.ObjectMapper;
import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.baggage.Baggage;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.SpanBuilder;
import io.opentelemetry.api.trace.SpanContext;
import io.opentelemetry.api.trace.SpanKind;
import io.opentelemetry.api.trace.TraceFlags;
import io.opentelemetry.api.trace.TraceState;
import io.opentelemetry.api.trace.Tracer;
import io.opentelemetry.context.Context;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Map;
import org.springframework.amqp.AmqpRejectAndDontRequeueException;
import org.springframework.http.HttpHeaders;
import org.springframework.amqp.core.Message;
import org.springframework.amqp.core.ReturnedMessage;
import org.junit.jupiter.api.Test;
import org.springframework.amqp.core.Queue;
import org.springframework.amqp.core.TopicExchange;
import org.springframework.amqp.core.MessageProperties;
import org.springframework.amqp.rabbit.connection.CorrelationData;
import org.springframework.amqp.rabbit.core.RabbitTemplate;
import org.springframework.amqp.rabbit.listener.RabbitListenerEndpoint;
import org.springframework.amqp.rabbit.listener.SimpleMessageListenerContainer;
import org.springframework.amqp.listener.ListenerExecutionFailedException;
import org.springframework.test.util.ReflectionTestUtils;
import org.mockito.Answers;
import org.mockito.MockedStatic;

class OrderProducerTest {
    @Test
    void declares_durable_topic_queues_with_dead_letter_routes() {
        RabbitMessagingConfiguration configuration = new RabbitMessagingConfiguration();
        TopicExchange events = configuration.commerceEventsExchange();
        Queue orders = configuration.fulfillmentOrdersQueue();
        Queue analytics = configuration.fulfillmentAnalyticsQueue();
        Queue analyticsDead = configuration.fulfillmentAnalyticsDeadLetterQueue();
        Queue ordersRetry = configuration.fulfillmentOrdersRetryQueue();
        Queue analyticsRetry = configuration.fulfillmentAnalyticsRetryQueue();
        TopicExchange retryReturn = configuration.commerceRetryReturnExchange();

        assertTrue(events.isDurable());
        assertTrue(orders.isDurable());
        assertTrue(analytics.isDurable());
        assertEquals("analytics.events", analytics.getName());
        assertEquals("analytics.events.dead", analyticsDead.getName());
        assertTrue(retryReturn.isDurable());
        assertEquals(
            RabbitMessagingConfiguration.DEAD_LETTER_EXCHANGE,
            orders.getArguments().get("x-dead-letter-exchange")
        );
        assertEquals(
            RabbitMessagingConfiguration.ORDERS_DEAD_LETTER_ROUTE,
            orders.getArguments().get("x-dead-letter-routing-key")
        );
        assertEquals(
            RabbitMessagingConfiguration.analyticsDeadLetterRoutingKey(analytics.getName()),
            analytics.getArguments().get("x-dead-letter-routing-key")
        );
        assertEquals(30_000, ordersRetry.getArguments().get("x-message-ttl"));
        assertEquals(30_000, analyticsRetry.getArguments().get("x-message-ttl"));
        assertEquals(
            RabbitMessagingConfiguration.RETRY_RETURN_EXCHANGE,
            ordersRetry.getArguments().get("x-dead-letter-exchange")
        );
        assertEquals(
            RabbitMessagingConfiguration.RETRY_RETURN_EXCHANGE,
            analyticsRetry.getArguments().get("x-dead-letter-exchange")
        );
        assertEquals(
            RabbitMessagingConfiguration.RETRY_RETURN_EXCHANGE,
            configuration.orderRetryReturnBinding(orders, retryReturn).getExchange()
        );
        assertEquals(
            RabbitMessagingConfiguration.RETRY_RETURN_EXCHANGE,
            configuration.analyticsRetryReturnBinding(analytics, retryReturn).getExchange()
        );
        assertEquals(
            "analytics.events.retry",
            configuration.analyticsRetryReturnBinding(analytics, retryReturn).getRoutingKey()
        );
        assertEquals(
            "analytics.events.retry",
            configuration.analyticsRetryBinding(analyticsRetry, configuration.commerceRetryExchange())
                .getRoutingKey()
        );
    }

    @Test
    void derives_analytics_retry_and_dead_letter_routes_from_a_custom_queue_name() {
        RabbitMessagingConfiguration configuration = new RabbitMessagingConfiguration();
        ReflectionTestUtils.setField(configuration, "analyticsQueueName", "analytics.custom");

        Queue analytics = configuration.fulfillmentAnalyticsQueue();
        Queue analyticsDead = configuration.fulfillmentAnalyticsDeadLetterQueue();
        Queue analyticsRetry = configuration.fulfillmentAnalyticsRetryQueue();
        TopicExchange retryReturn = configuration.commerceRetryReturnExchange();

        assertEquals("analytics.custom", analytics.getName());
        assertEquals("analytics.custom.dead", analyticsDead.getName());
        assertEquals(
            "analytics.custom.dead",
            analytics.getArguments().get("x-dead-letter-routing-key")
        );
        assertEquals(
            "analytics.custom.retry",
            analyticsRetry.getArguments().get("x-dead-letter-routing-key")
        );
        assertEquals(
            "analytics.custom.retry",
            configuration.analyticsRetryReturnBinding(analytics, retryReturn).getRoutingKey()
        );
        assertEquals(
            "analytics.custom.retry",
            configuration.analyticsRetryBinding(analyticsRetry, configuration.commerceRetryExchange())
                .getRoutingKey()
        );
    }

    @Test
    void broker_retry_count_leaves_the_final_delivery_for_durable_dead_lettering() {
        assertEquals(5, FulfillmentRepository.MAX_CLAIM_ATTEMPTS);
        assertEquals(4, RabbitMessagingConfiguration.BROKER_RETRY_COUNT);
    }

    @Test
    void round_trips_w3c_trace_state_and_baggage_through_amqp_headers() {
        SpanContext span = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.builder().put("vendor", "state").build()
        );
        Context source = Context.root()
            .with(Span.wrap(span))
            .with(Baggage.builder()
                .put("tenant.id", "tenant-acme")
                .put("user.tier", "gold")
                .put("customer.segment", "returning")
                .build());
        MessageProperties properties = new MessageProperties();

        RabbitTraceContext.inject(source, properties);
        Context extracted = RabbitTraceContext.extract(properties);

        SpanContext extractedSpan = Span.fromContext(extracted).getSpanContext();
        assertEquals(span.getTraceId(), extractedSpan.getTraceId());
        assertEquals(span.getSpanId(), extractedSpan.getSpanId());
        assertEquals("state", extractedSpan.getTraceState().get("vendor"));
        assertEquals("tenant-acme", Baggage.fromContext(extracted).getEntryValue("tenant.id"));
        assertEquals("gold", Baggage.fromContext(extracted).getEntryValue("user.tier"));
        assertEquals("returning", Baggage.fromContext(extracted).getEntryValue("customer.segment"));
        assertEquals(
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            properties.getHeaders().get("traceparent")
        );
        assertEquals("vendor=state", properties.getHeaders().get("tracestate"));
        assertTrue(properties.getHeaders().containsKey("baggage"));
    }

    @Test
    void fills_the_required_tracestate_when_the_incoming_context_has_none() {
        SpanContext span = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.builder().build()
        );
        MessageProperties properties = new MessageProperties();

        RabbitTraceContext.inject(
            Context.root().with(Span.wrap(span)).with(Baggage.builder()
                .put("tenant.id", "tenant-acme")
                .build()),
            properties
        );

        assertEquals("playground=commerce", properties.getHeaders().get("tracestate"));
        RabbitTraceContext.require(properties);
    }

    @Test
    void drops_unknown_and_oversized_baggage_at_the_amqp_boundary() {
        Context source = Context.root().with(Baggage.builder()
            .put("tenant.id", "tenant-acme")
            .put("secret.token", "do-not-forward")
            .put("customer.segment", "x".repeat(129))
            .build());
        MessageProperties properties = new MessageProperties();

        RabbitTraceContext.inject(source, properties);

        String baggage = String.valueOf(properties.getHeaders().get("baggage"));
        assertTrue(baggage.contains("tenant.id=tenant-acme"));
        assertFalse(baggage.contains("secret.token"));
        assertFalse(baggage.contains("customer.segment"));
    }

    @Test
    void rejects_malformed_inherited_baggage_before_injection() {
        MessageProperties properties = new MessageProperties();
        properties.setHeader("baggage", "tenant.id=%zz");

        assertThrows(
            PermanentEventException.class,
            () -> RabbitTraceContext.inject(validContext(), properties)
        );
        assertEquals("tenant.id=%zz", properties.getHeaders().get("baggage"));
    }

    @Test
    void rejects_duplicate_inherited_baggage_keys_before_republish() {
        RabbitTemplate rabbit = mock(RabbitTemplate.class);
        MessageProperties properties = new MessageProperties();
        properties.setMessageId("duplicate-baggage");
        properties.setReceivedExchange(RabbitMessagingConfiguration.EVENTS_EXCHANGE);
        properties.setReceivedRoutingKey("order.created");
        properties.setHeader("baggage", "tenant.id=tenant-acme,tenant.id=tenant-acme");
        Message source = new Message("{}".getBytes(StandardCharsets.UTF_8), properties);

        assertThrows(
            PermanentEventException.class,
            () -> new OrderEventPublisher(rabbit, new ObjectMapper()).defer(
                source,
                RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY,
                validContext()
            )
        );
        verifyNoInteractions(rabbit);
    }

    @Test
    void rejects_malformed_inherited_baggage_before_republish() {
        RabbitTemplate rabbit = mock(RabbitTemplate.class);
        MessageProperties properties = new MessageProperties();
        properties.setMessageId("malformed-baggage");
        properties.setReceivedExchange(RabbitMessagingConfiguration.EVENTS_EXCHANGE);
        properties.setReceivedRoutingKey("order.created");
        properties.setHeader("baggage", "tenant.id=%zz");
        Message source = new Message("{}".getBytes(StandardCharsets.UTF_8), properties);

        assertThrows(
            PermanentEventException.class,
            () -> new OrderEventPublisher(rabbit, new ObjectMapper()).defer(
                source,
                RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY,
                validContext()
            )
        );
        verifyNoInteractions(rabbit);
    }

    @Test
    void rejects_conflicting_inherited_baggage_before_republish() {
        RabbitTemplate rabbit = mock(RabbitTemplate.class);
        MessageProperties properties = new MessageProperties();
        properties.setMessageId("conflicting-baggage");
        properties.setReceivedExchange(RabbitMessagingConfiguration.EVENTS_EXCHANGE);
        properties.setReceivedRoutingKey("order.created");
        properties.setHeader("baggage", "tenant.id=another-tenant");
        Message source = new Message("{}".getBytes(StandardCharsets.UTF_8), properties);

        assertThrows(
            PermanentEventException.class,
            () -> new OrderEventPublisher(rabbit, new ObjectMapper()).defer(
                source,
                RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY,
                validContext()
            )
        );
        verifyNoInteractions(rabbit);
    }

    @Test
    void removes_stale_w3c_headers_from_message_and_http_carriers_for_root_context() {
        MessageProperties properties = new MessageProperties();
        properties.setHeader("TraceParent", "stale-traceparent");
        properties.setHeader("TRACESTATE", "stale-tracestate");
        properties.setHeader("Baggage", "tenant.id=stale");

        RabbitTraceContext.inject(Context.root(), properties);

        assertFalse(properties.getHeaders().keySet().stream()
            .anyMatch(key -> key.equalsIgnoreCase("traceparent")));
        assertFalse(properties.getHeaders().keySet().stream()
            .anyMatch(key -> key.equalsIgnoreCase("tracestate")));
        assertFalse(properties.getHeaders().keySet().stream()
            .anyMatch(key -> key.equalsIgnoreCase("baggage")));

        HttpHeaders headers = new HttpHeaders();
        headers.add("TraceParent", "stale-traceparent");
        headers.add("TRACESTATE", "stale-tracestate");
        headers.add("Baggage", "stale-baggage");

        RabbitTraceContext.inject(Context.root(), headers);

        assertNull(headers.get("traceparent"));
        assertNull(headers.get("tracestate"));
        assertNull(headers.get("baggage"));
    }

    @Test
    void enforces_baggage_limit_in_utf8_bytes_for_non_ascii_values() {
        Context atLimit = Context.root().with(Baggage.builder()
            .put("customer.segment", "é".repeat(64))
            .build());
        Context overLimit = Context.root().with(Baggage.builder()
            .put("customer.segment", "é".repeat(65))
            .build());
        MessageProperties atLimitProperties = new MessageProperties();
        MessageProperties overLimitProperties = new MessageProperties();

        RabbitTraceContext.inject(atLimit, atLimitProperties);
        RabbitTraceContext.inject(overLimit, overLimitProperties);

        assertEquals(128, "é".repeat(64).getBytes(StandardCharsets.UTF_8).length);
        assertTrue(String.valueOf(atLimitProperties.getHeaders().get("baggage"))
            .contains("customer.segment="));
        assertFalse(String.valueOf(overLimitProperties.getHeaders().get("baggage"))
            .contains("customer.segment="));
    }

    @Test
    void parses_itemized_order_event_without_a_service_stub() {
        CommerceEvent event = CommerceEvent.parse(
            new ObjectMapper(),
            """
            {
              "event_id":"evt-1",
              "event_key":"order.created:order-1",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.created",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-1",
              "payload":{
                "order_id":"order-1",
                "customer_id":"customer-acme-ava",
                "currency":"USD",
                "total_minor":9177,
                "payment_status":"captured",
                "items":[{"sku":"WIDGET-1","quantity":2}]
              }
            }
            """.getBytes(java.nio.charset.StandardCharsets.UTF_8)
        );

        assertEquals("order-1", event.orderId());
        assertEquals("captured", event.paymentStatus());
        assertTrue(event.isFulfillmentTrigger());
        assertEquals("fulfillment.shipment.created", event.shipmentCreated(
            new ObjectMapper(), "shipment-order-1-1", "label_created"
        ).eventType());
    }

    @Test
    void publisher_sends_persistent_json_with_explicit_producer_semantics() {
        RabbitTemplate rabbit = mock(RabbitTemplate.class);
        doAnswer(invocation -> {
            CorrelationData correlation = invocation.getArgument(3);
            correlation.getFuture().complete(new CorrelationData.Confirm(true, null));
            return null;
        }).when(rabbit).send(
            eq(RabbitMessagingConfiguration.EVENTS_EXCHANGE),
            eq("fulfillment.shipment.created"),
            any(Message.class),
            any(CorrelationData.class)
        );
        CommerceEvent event = CommerceEvent.parse(
            new ObjectMapper(),
            """
            {
              "event_id":"shipment-event-1",
              "event_key":"order.created:order-1:shipment",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"fulfillment.shipment.created",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-1",
              "payload":{"order_id":"order-1"}
            }
            """.getBytes(StandardCharsets.UTF_8)
        );
        SpanContext parent = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.builder().put("vendor", "state").build()
        );
        Context source = Context.root()
            .with(Span.wrap(parent))
            .with(Baggage.builder()
                .put("tenant.id", "tenant-acme")
                .put("user.tier", "standard")
                .put("customer.segment", "standard")
                .put("region", "us-east-1")
                .put("request.priority", "normal")
                .put("session.id", "session-shipment-1")
                .build());

        Tracer tracer = mock(Tracer.class);
        SpanBuilder spanBuilder = mock(SpanBuilder.class);
        SpanContext producerSpanContext = SpanContext.createFromRemoteParent(
            parent.getTraceId(),
            "0f1e2d3c4b5a6978",
            TraceFlags.getSampled(),
            parent.getTraceState()
        );
        Span producerSpan = Span.wrap(producerSpanContext);
        Map<String, String> producerAttributes = new HashMap<>();
        when(tracer.spanBuilder("commerce.events publish")).thenReturn(spanBuilder);
        when(spanBuilder.setParent(any(Context.class))).thenReturn(spanBuilder);
        when(spanBuilder.setSpanKind(SpanKind.PRODUCER)).thenReturn(spanBuilder);
        doAnswer(invocation -> {
            producerAttributes.put(invocation.getArgument(0), invocation.getArgument(1));
            return spanBuilder;
        }).when(spanBuilder).setAttribute(anyString(), anyString());
        when(spanBuilder.startSpan()).thenReturn(producerSpan);

        try (MockedStatic<GlobalOpenTelemetry> telemetry = org.mockito.Mockito.mockStatic(
            GlobalOpenTelemetry.class,
            Answers.CALLS_REAL_METHODS
        )) {
            telemetry.when(() -> GlobalOpenTelemetry.getTracer("dev.tailrocks.fulfillment"))
                .thenReturn(tracer);
            new OrderEventPublisher(rabbit, new ObjectMapper()).publish(event, source);
        }

        var sent = forClass(Message.class);
        verify(rabbit).send(
            eq(RabbitMessagingConfiguration.EVENTS_EXCHANGE),
            eq("fulfillment.shipment.created"),
            sent.capture(),
            any(CorrelationData.class)
        );
        verify(spanBuilder).setSpanKind(SpanKind.PRODUCER);
        assertEquals("producer", producerAttributes.get("otel.kind"));
        assertEquals("rabbitmq", producerAttributes.get("messaging.system"));
        assertEquals("commerce.events", producerAttributes.get("messaging.destination.name"));
        assertEquals("send", producerAttributes.get("messaging.operation.name"));
        assertEquals(event.eventId(), producerAttributes.get("messaging.message.id"));
        assertEquals("fulfillment.shipment.created", producerAttributes.get("commerce.event.type"));
        assertEquals("tenant-acme", producerAttributes.get("tenant.id"));
        assertEquals("standard", producerAttributes.get("user.tier"));
        assertEquals("standard", producerAttributes.get("customer.segment"));
        assertEquals("us-east-1", producerAttributes.get("region"));
        assertEquals("normal", producerAttributes.get("request.priority"));
        assertEquals("session-shipment-1", producerAttributes.get("session.id"));
        assertEquals(12, producerAttributes.size());
        assertEquals(event.eventId(), sent.getValue().getMessageProperties().getMessageId());
        Context extracted = RabbitTraceContext.extract(sent.getValue().getMessageProperties());
        SpanContext extractedSpan = Span.fromContext(extracted).getSpanContext();
        assertEquals(producerSpanContext.getTraceId(), extractedSpan.getTraceId());
        assertEquals("state", extractedSpan.getTraceState().get("vendor"));
        assertEquals("tenant-acme", Baggage.fromContext(extracted).getEntryValue("tenant.id"));
        assertEquals("standard", Baggage.fromContext(extracted).getEntryValue("user.tier"));
        assertEquals(
            "standard",
            Baggage.fromContext(extracted).getEntryValue("customer.segment")
        );
        assertEquals("us-east-1", Baggage.fromContext(extracted).getEntryValue("region"));
        assertEquals("normal", Baggage.fromContext(extracted).getEntryValue("request.priority"));
        assertEquals(
            "session-shipment-1",
            Baggage.fromContext(extracted).getEntryValue("session.id")
        );
        assertTrue(sent.getValue().getMessageProperties().getHeaders().containsKey("traceparent"));
        assertTrue(sent.getValue().getMessageProperties().getHeaders().containsKey("tracestate"));
        assertTrue(sent.getValue().getMessageProperties().getHeaders().containsKey("baggage"));
        assertEquals(
            "customer.segment=standard,region=us-east-1,request.priority=normal,"
                + "session.id=session-shipment-1,tenant.id=tenant-acme,user.tier=standard",
            sent.getValue().getMessageProperties().getHeaders().get("baggage")
        );
    }

    @Test
    void lease_retry_publisher_reinjects_w3c_headers_on_the_real_amqp_message() {
        RabbitTemplate rabbit = mock(RabbitTemplate.class);
        doAnswer(invocation -> {
            CorrelationData correlation = invocation.getArgument(3);
            correlation.getFuture().complete(new CorrelationData.Confirm(true, null));
            return null;
        }).when(rabbit).send(
            eq(RabbitMessagingConfiguration.RETRY_EXCHANGE),
            eq(RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY),
            any(Message.class),
            any(CorrelationData.class)
        );
        SpanContext parent = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.builder().put("vendor", "state").build()
        );
        Context source = Context.root()
            .with(Span.wrap(parent))
            .with(Baggage.builder().put("tenant.id", "tenant-acme").build());
        MessageProperties originalProperties = new MessageProperties();
        originalProperties.setMessageId("event-lease-retry");
        originalProperties.setReceivedExchange(RabbitMessagingConfiguration.EVENTS_EXCHANGE);
        originalProperties.setReceivedRoutingKey("order.created");
        originalProperties.setHeader("baggage", "tenant.id=tenant%2Dacme");
        Message original = new Message("{}".getBytes(StandardCharsets.UTF_8), originalProperties);

        new OrderEventPublisher(rabbit, new ObjectMapper()).defer(
            original,
            RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY,
            source
        );

        var sent = forClass(Message.class);
        verify(rabbit).send(
            eq(RabbitMessagingConfiguration.RETRY_EXCHANGE),
            eq(RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY),
            sent.capture(),
            any(CorrelationData.class)
        );
        Context extracted = RabbitTraceContext.extract(sent.getValue().getMessageProperties());
        SpanContext extractedSpan = Span.fromContext(extracted).getSpanContext();
        assertEquals(parent.getTraceId(), extractedSpan.getTraceId());
        assertEquals("state", extractedSpan.getTraceState().get("vendor"));
        assertEquals(
            "tenant-acme",
            Baggage.fromContext(extracted).getEntryValue("tenant.id")
        );
        assertTrue(sent.getValue().getMessageProperties().getHeaders().containsKey("traceparent"));
        assertTrue(sent.getValue().getMessageProperties().getHeaders().containsKey("tracestate"));
        assertTrue(sent.getValue().getMessageProperties().getHeaders().containsKey("baggage"));
        assertEquals(
            "tenant.id=tenant-acme",
            sent.getValue().getMessageProperties().getHeaders().get("baggage")
        );
    }

    @Test
    void listener_factory_applies_configured_prefetch_and_concurrency() {
        RabbitMessagingConfiguration configuration = new RabbitMessagingConfiguration();
        ReflectionTestUtils.setField(configuration, "listenerPrefetch", 37);
        ReflectionTestUtils.setField(configuration, "listenerConcurrency", 2);
        ReflectionTestUtils.setField(configuration, "listenerMaxConcurrency", 6);

        var factory = configuration.rabbitListenerContainerFactory(
            mock(org.springframework.amqp.rabbit.connection.ConnectionFactory.class),
            configuration.rabbitRetryInterceptor()
        );
        RabbitListenerEndpoint endpoint = mock(RabbitListenerEndpoint.class);
        SimpleMessageListenerContainer container = factory.createListenerContainer(endpoint);

        assertEquals(37, ReflectionTestUtils.getField(container, "prefetchCount"));
        assertEquals(2, ReflectionTestUtils.getField(container, "concurrentConsumers"));
        assertEquals(6, ReflectionTestUtils.getField(container, "maxConcurrentConsumers"));
    }

    @Test
    void retry_recoverer_marks_manual_delivery_for_broker_rejection() {
        Message message = new Message("{}".getBytes(StandardCharsets.UTF_8), new MessageProperties());

        ListenerExecutionFailedException failure = assertThrows(
            ListenerExecutionFailedException.class,
            () -> RabbitMessagingConfiguration.manualRejectingRecoverer().recover(
                message,
                new IllegalStateException("downstream unavailable")
            )
        );

        AmqpRejectAndDontRequeueException rejection = assertInstanceOf(
            AmqpRejectAndDontRequeueException.class,
            failure.getCause()
        );
        assertTrue(rejection.isRejectManual());
    }

    @Test
    void rabbit_template_enables_confirm_and_return_listeners() {
        RabbitMessagingConfiguration configuration = new RabbitMessagingConfiguration();

        RabbitTemplate template = configuration.rabbitTemplate(
            mock(org.springframework.amqp.rabbit.connection.ConnectionFactory.class)
        );

        assertTrue(template.isConfirmListener());
        assertTrue(template.isReturnListener());
    }

    @Test
    void publisher_rejects_a_positive_confirm_when_rabbit_returns_the_message() {
        RabbitTemplate rabbit = mock(RabbitTemplate.class);
        doAnswer(invocation -> {
            CorrelationData correlation = invocation.getArgument(3);
            MessageProperties returnedProperties = new MessageProperties();
            returnedProperties.setMessageId("returned-event");
            correlation.setReturned(new ReturnedMessage(
                new Message("{}".getBytes(StandardCharsets.UTF_8), returnedProperties),
                312,
                "NO_ROUTE",
                RabbitMessagingConfiguration.EVENTS_EXCHANGE,
                "missing.route"
            ));
            correlation.getFuture().complete(new CorrelationData.Confirm(true, null));
            return null;
        }).when(rabbit).send(
            eq(RabbitMessagingConfiguration.EVENTS_EXCHANGE),
            eq("fulfillment.shipment.created"),
            any(Message.class),
            any(CorrelationData.class)
        );
        CommerceEvent event = CommerceEvent.parse(
            new ObjectMapper(),
            """
            {
              "event_id":"returned-event",
              "event_key":"order.created:order-returned:shipment",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"fulfillment.shipment.created",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-returned",
              "payload":{"order_id":"order-returned"}
            }
            """.getBytes(StandardCharsets.UTF_8)
        );

        RetryableEventException failure = assertThrows(
            RetryableEventException.class,
            () -> new OrderEventPublisher(rabbit, new ObjectMapper()).publish(event, validContext())
        );

        assertTrue(failure.getMessage().contains("NO_ROUTE"));
    }

    @Test
    void rabbit_event_context_requires_all_three_valid_w3c_headers() {
        MessageProperties properties = new MessageProperties();
        properties.setHeader(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        );
        properties.setHeader("tracestate", "vendor=state");
        properties.setHeader("baggage", "tenant.id=tenant-acme");

        RabbitTraceContext.require(properties);

        for (String header : new String[] {"traceparent", "tracestate", "baggage"}) {
            MessageProperties missing = new MessageProperties();
            missing.getHeaders().putAll(properties.getHeaders());
            missing.getHeaders().remove(header);
            assertThrows(PermanentEventException.class, () -> RabbitTraceContext.require(missing));
        }
    }

    @Test
    void rabbit_event_context_matches_w3c_tracestate_member_rules() {
        MessageProperties valid = new MessageProperties();
        valid.setHeader(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        );
        valid.setHeader("tracestate", "vendor/name=state");
        valid.setHeader("baggage", "tenant.id=tenant-acme");
        RabbitTraceContext.require(valid);

        String maxMember = "k=" + "v".repeat(254);
        assertEquals(256, maxMember.getBytes(StandardCharsets.UTF_8).length);
        valid.setHeader("tracestate", maxMember);
        RabbitTraceContext.require(valid);

        String tooLongMember = "k=" + "v".repeat(255);
        assertEquals(257, tooLongMember.getBytes(StandardCharsets.UTF_8).length);

        for (String tracestate : new String[] {
            "vendor=state,",
            ",vendor=state",
            "vendor=state,,other=value",
            tooLongMember,
            "k=",
            "k==state",
            "K=state",
            "vendor=state,vendor=other"
        }) {
            MessageProperties invalid = new MessageProperties();
            invalid.getHeaders().putAll(valid.getHeaders());
            invalid.setHeader("tracestate", tracestate);
            assertThrows(PermanentEventException.class, () -> RabbitTraceContext.require(invalid));
        }
    }

    private static Context validContext() {
        SpanContext span = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.builder().put("vendor", "state").build()
        );
        return Context.root()
            .with(Span.wrap(span))
            .with(Baggage.builder().put("tenant.id", "tenant-acme").build());
    }
}
