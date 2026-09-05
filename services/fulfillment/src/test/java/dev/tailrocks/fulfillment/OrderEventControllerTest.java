package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

import com.fasterxml.jackson.databind.ObjectMapper;
import java.util.Map;
import java.util.Optional;
import org.junit.jupiter.api.Test;
import org.springframework.http.HttpStatus;

class OrderEventControllerTest {
    private static final String TOKEN = "fulfillment-test-token";

    @Test
    void rejects_an_unconfigured_internal_endpoint_at_startup() {
        OrderEventController controller = controller("");

        assertThrows(IllegalStateException.class, controller::validateInternalToken);
    }

    @Test
    void requires_the_internal_bearer_or_explicit_header() {
        OrderEventController controller = controller(TOKEN);

        var response = controller.verify(
            "order-1",
            "tenant-acme",
            null,
            null,
            null
        );

        assertEquals(HttpStatus.UNAUTHORIZED, response.getStatusCode());
    }

    @Test
    void requires_a_tenant_and_rejects_conflicting_tenant_sources() {
        OrderEventController controller = controller(TOKEN);

        var missing = controller.verify("order-1", null, "Bearer " + TOKEN, null, null);
        var conflicting = controller.verify(
            "order-1",
            "tenant-acme",
            "Bearer " + TOKEN,
            null,
            "tenant-other"
        );

        assertEquals(HttpStatus.BAD_REQUEST, missing.getStatusCode());
        assertEquals(HttpStatus.BAD_REQUEST, conflicting.getStatusCode());
    }

    @Test
    void verifies_only_the_requested_tenant_order() {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        OrderEventController controller = new OrderEventController(repository, publisher, TOKEN);
        when(repository.verifyOrder("tenant-acme", "order-1")).thenReturn(Optional.of(Map.ofEntries(
            Map.entry("tenant_id", "tenant-acme"),
            Map.entry("order_id", "order-1"),
            Map.entry("order_status", "processing"),
            Map.entry("shipment_id", "shipment-1"),
            Map.entry("shipment_status", "label_created"),
            Map.entry("fulfillment_status", "completed"),
            Map.entry("notification_deliveries", 1L),
            Map.entry("notification_status", "delivered"),
            Map.entry("notification_channel", "in_app"),
            Map.entry("notification_provider", "in_app_durable_sink"),
            Map.entry("notification_acknowledgement", "in-app")
        )));

        var response = controller.verify(
            "order-1",
            "tenant-acme",
            null,
            TOKEN,
            null
        );

        assertEquals(HttpStatus.OK, response.getStatusCode());
        assertEquals(true, response.getBody().get("ready"));
        assertEquals("delivered", response.getBody().get("notification_status"));
        assertEquals("in_app", response.getBody().get("notification_channel"));
        assertEquals("in_app_durable_sink", response.getBody().get("notification_provider"));
        verify(repository).verifyOrder("tenant-acme", "order-1");
    }

    @Test
    void does_not_report_queued_notification_as_ready() {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        OrderEventController controller = new OrderEventController(repository, publisher, TOKEN);
        when(repository.verifyOrder("tenant-acme", "order-queued")).thenReturn(Optional.of(Map.ofEntries(
            Map.entry("tenant_id", "tenant-acme"),
            Map.entry("order_id", "order-queued"),
            Map.entry("order_status", "processing"),
            Map.entry("shipment_id", "shipment-queued"),
            Map.entry("shipment_status", "label_created"),
            Map.entry("fulfillment_status", "completed"),
            Map.entry("notification_deliveries", 1L),
            Map.entry("notification_status", "queued"),
            Map.entry("notification_channel", "in_app"),
            Map.entry("notification_provider", "in_app_durable_sink"),
            Map.entry("notification_acknowledgement", "missing")
        )));

        var response = controller.verify(
            "order-queued",
            "tenant-acme",
            "Bearer " + TOKEN,
            null,
            null
        );

        assertEquals(HttpStatus.OK, response.getStatusCode());
        assertEquals(false, response.getBody().get("ready"));
        assertEquals("queued", response.getBody().get("notification_status"));
    }

    @Test
    void does_not_report_cancelled_fulfillment_as_ready() {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        OrderEventController controller = new OrderEventController(repository, publisher, TOKEN);
        when(repository.verifyOrder("tenant-acme", "order-cancelled"))
            .thenReturn(Optional.of(Map.of(
                "tenant_id", "tenant-acme",
                "order_id", "order-cancelled",
                "order_status", "cancelled",
                "shipment_id", "shipment-cancelled",
                "shipment_status", "cancelled",
                "fulfillment_status", "completed",
                "notification_deliveries", 1L
            )));

        var response = controller.verify(
            "order-cancelled",
            "tenant-acme",
            "Bearer " + TOKEN,
            null,
            null
        );

        assertEquals(HttpStatus.OK, response.getStatusCode());
        assertEquals(false, response.getBody().get("ready"));
    }

    @Test
    void publishes_only_after_authenticated_tenant_scoped_lookup() {
        FulfillmentRepository repository = mock(FulfillmentRepository.class);
        OrderEventPublisher publisher = mock(OrderEventPublisher.class);
        OrderEventController controller = new OrderEventController(repository, publisher, TOKEN);
        CommerceEvent event = CommerceEvent.parse(new ObjectMapper(), """
            {
              "event_id":"event-1",
              "event_key":"order.created:order-1",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.created",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-1",
              "payload":{"order_id":"order-1"}
            }
            """.getBytes(java.nio.charset.StandardCharsets.UTF_8));
        when(repository.loadOrderEvent("tenant-acme", "order-1"))
            .thenReturn(Optional.of(event));

        var response = controller.publish(
            "order-1",
            "tenant-acme",
            "Bearer " + TOKEN,
            null,
            null
        );

        assertEquals(HttpStatus.OK, response.getStatusCode());
        verify(repository).loadOrderEvent("tenant-acme", "order-1");
        verify(publisher).publish(event, io.opentelemetry.context.Context.current());
    }

    private static OrderEventController controller(String token) {
        return new OrderEventController(
            mock(FulfillmentRepository.class),
            mock(OrderEventPublisher.class),
            token
        );
    }
}
