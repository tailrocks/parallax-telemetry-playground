package dev.tailrocks.fulfillment;

import static org.mockito.ArgumentMatchers.any;
import static org.mockito.ArgumentMatchers.eq;
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.doThrow;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.never;
import static org.mockito.Mockito.times;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.rabbitmq.client.Channel;
import io.opentelemetry.context.Context;
import java.nio.charset.StandardCharsets;
import org.junit.jupiter.api.Test;
import org.springframework.amqp.core.Message;
import org.springframework.amqp.core.MessageProperties;

class AnalyticsConsumerTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void preserves_an_explicit_session_id_in_the_versioned_event_envelope() {
        CommerceEvent event = CommerceEvent.parse(MAPPER, """
            {
              "event_id":"event-session-1",
              "event_key":"order.paid:order-session-1",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.paid",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-session-1",
              "session_id":"session-explicit",
              "payload":{"order_id":"order-session-1"}
            }
            """.getBytes(StandardCharsets.UTF_8));

        assertEquals("session-explicit", event.sessionId());
        assertTrue(event.toJson(MAPPER).contains("\"session_id\":\"session-explicit\""));
    }

    @Test
    void replays_clickhouse_ingestion_after_a_claimed_delivery_fails() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        ClickHouseAnalyticsClient clickhouse = mock(ClickHouseAnalyticsClient.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        Channel channel = mock(Channel.class);
        AnalyticsEventConsumer consumer = new AnalyticsEventConsumer(
            MAPPER,
            repository,
            clickhouse,
            publisher,
            "analytics.events"
        );
        CommerceEvent event = event("order.paid", "order-analytics-1");
        AnalyticsClaim claimed = AnalyticsClaim.acquired(
            "analytics-1",
            "trace-1",
            "span-1",
            "lease-analytics-1"
        );
        when(repository.claimAnalytics(any())).thenReturn(claimed);
        stubHeartbeat(repository);
        RuntimeException failure = new RetryableEventException("ClickHouse unavailable");
        doThrow(failure).doNothing().when(clickhouse).ingest(any());

        org.junit.jupiter.api.Assertions.assertThrows(
            RetryableEventException.class,
            () -> consumer.onEvent(message(event, 51), channel)
        );
        consumer.onEvent(message(event, 52), channel);

        verify(repository).markFailed(
            FulfillmentRepository.ANALYTICS_CONSUMER,
            event.tenantId(),
            event.eventKey(),
            "lease-analytics-1",
            failure
        );
        verify(repository).markCompleted(
            event.tenantId(),
            FulfillmentRepository.ANALYTICS_CONSUMER,
            event.eventKey(),
            "lease-analytics-1"
        );
        verify(clickhouse, times(2)).ingest(any());
        verify(channel).basicAck(52, false);
        verify(channel, never()).basicAck(51, false);
    }

    @Test
    void defers_an_active_analytics_claim_until_its_lease_can_be_reclaimed() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        ClickHouseAnalyticsClient clickhouse = mock(ClickHouseAnalyticsClient.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        Channel channel = mock(Channel.class);
        AnalyticsEventConsumer consumer = new AnalyticsEventConsumer(
            MAPPER,
            repository,
            clickhouse,
            publisher,
            "analytics.events"
        );
        CommerceEvent event = event("order.paid", "order-analytics-2");
        when(repository.claimAnalytics(any())).thenReturn(AnalyticsClaim.activeLease());

        consumer.onEvent(message(event, 53), channel);

        verify(publisher).defer(
            any(Message.class),
            eq(RabbitMessagingConfiguration.analyticsRetryRoutingKey("analytics.events")),
            any(Context.class)
        );
        verify(channel).basicAck(53, false);
        verify(clickhouse, never()).ingest(any());
        verify(repository, never()).markCompleted(any(), any(), any(), any());
        verify(repository, never()).markFailed(any(), any(), any(), any(), any());
    }

    @Test
    void records_a_claimed_permanent_analytics_failure_before_rejecting() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        ClickHouseAnalyticsClient clickhouse = mock(ClickHouseAnalyticsClient.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        Channel channel = mock(Channel.class);
        AnalyticsEventConsumer consumer = new AnalyticsEventConsumer(
            MAPPER,
            repository,
            clickhouse,
            publisher,
            "analytics.events"
        );
        CommerceEvent event = event("order.paid", "order-analytics-permanent");
        AnalyticsClaim claim = AnalyticsClaim.acquired(
            "analytics-permanent",
            "trace-permanent",
            "span-permanent",
            "lease-analytics-permanent"
        );
        when(repository.claimAnalytics(any())).thenReturn(claim);
        stubHeartbeat(repository);
        PermanentEventException failure = new PermanentEventException("invalid analytics payload");
        doThrow(failure).when(clickhouse).ingest(any());

        consumer.onEvent(message(event, 54), channel);

        verify(repository).markFailed(
            eq(FulfillmentRepository.ANALYTICS_CONSUMER),
            eq(event.tenantId()),
            eq(event.eventKey()),
            eq("lease-analytics-permanent"),
            eq(failure)
        );
        verify(channel).basicReject(54, false);
    }

    @Test
    void rejects_an_analytics_event_without_complete_w3c_context() throws Exception {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        AnalyticsEventConsumer consumer = new AnalyticsEventConsumer(
            MAPPER,
            repository,
            mock(ClickHouseAnalyticsClient.class),
            mock(OrderEventPublisher.class),
            "analytics.events"
        );
        Channel channel = mock(Channel.class);
        Message message = message(event("order.paid", "order-context-missing"), 55);
        message.getMessageProperties().getHeaders().remove("baggage");

        consumer.onEvent(message, channel);

        verify(channel).basicReject(55, false);
        verify(repository, never()).claimAnalytics(any());
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

    private static CommerceEvent event(String type, String orderId) {
        return CommerceEvent.parse(MAPPER, ("""
            {
              "event_id":"event-%s",
              "event_key":"%s:%s",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"%s",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"%s",
              "payload":{
                "order_id":"%s",
                "customer_id":"customer-acme-ava",
                "currency":"USD",
                "total_minor":9177,
                "payment_status":"captured"
              }
            }
            """).formatted(type, type, orderId, type, orderId, orderId)
            .getBytes(StandardCharsets.UTF_8));
    }
}
