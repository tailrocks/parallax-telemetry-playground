package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import org.junit.jupiter.api.Test;

class CommerceEventTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void requires_a_versioned_object_envelope_and_does_not_fallback_to_root_payload()
        throws Exception {
        ObjectNode root = (ObjectNode) MAPPER.readTree(validJson());
        root.remove("payload");
        String missingPayload = MAPPER.writeValueAsString(root);
        String missingEventId = validJson().replace("\"event_id\":\"event-1\",", "");
        String unsupportedVersion = validJson().replace("\"schema_version\":1", "\"schema_version\":2");
        String duplicateRootField = validJson().replace(
            "\"event_id\":\"event-1\",",
            "\"event_id\":\"event-1\",\"event_id\":\"event-2\","
        );
        String unknownRootField = validJson().replace(
            "\"payload\":{\"order_id\":\"order-1\"}",
            "\"unexpected\":true,\"payload\":{\"order_id\":\"order-1\"}"
        );
        String mismatchedAggregate = validJson().replace(
            "\"aggregate_id\":\"order-1\"",
            "\"aggregate_id\":\"order-2\""
        );

        assertThrows(PermanentEventException.class, () -> parse(missingPayload));
        assertThrows(PermanentEventException.class, () -> parse(missingEventId));
        assertThrows(PermanentEventException.class, () -> parse(unsupportedVersion));
        assertThrows(PermanentEventException.class, () -> parse(duplicateRootField));
        assertThrows(PermanentEventException.class, () -> parse(unknownRootField));
        assertThrows(PermanentEventException.class, () -> parse(mismatchedAggregate));

        CommerceEvent aggregateOnly = parse(validJson().replace(
            "\"payload\":{\"order_id\":\"order-1\"}",
            "\"payload\":{}"
        ));
        assertEquals("order-1", aggregateOnly.orderId());
    }

    @Test
    void validates_rabbit_identity_headers_against_the_body() {
        Map<String, Object> headers = Map.of(
            "event-key", "order.created:order-1",
            "event-type", "order.created",
            "tenant-id", "tenant-acme",
            "message-id", "event-1"
        );

        CommerceEvent event = CommerceEvent.parse(
            MAPPER,
            validJson().getBytes(StandardCharsets.UTF_8),
            headers,
            "event-1"
        );

        assertEquals("event-1", event.eventId());
        assertThrows(
            PermanentEventException.class,
            () -> CommerceEvent.parse(
                MAPPER,
                validJson().getBytes(StandardCharsets.UTF_8),
                Map.of(
                    "event-key", "different-key",
                    "event-type", "order.created",
                    "tenant-id", "tenant-acme"
                ),
                "event-1"
            )
        );
        assertThrows(
            PermanentEventException.class,
            () -> CommerceEvent.parse(
                MAPPER,
                validJson().getBytes(StandardCharsets.UTF_8),
                Map.of("event-key", "order.created:order-1"),
                "event-1"
            )
        );
        assertThrows(
            PermanentEventException.class,
            () -> CommerceEvent.parse(
                MAPPER,
                validJson().getBytes(StandardCharsets.UTF_8),
                headers,
                "different-event"
            )
        );
    }

    @Test
    void uses_one_durable_effect_key_for_all_fulfillment_triggers() {
        CommerceEvent created = parse(validJson());
        CommerceEvent paid = parse(validJson()
            .replace("event-1", "event-2")
            .replace("order.created:order-1", "order.paid:order-1")
            .replace("order.created", "order.paid"));
        CommerceEvent captured = parse(paymentJson("payment.captured", "captured"));

        assertEquals(created.processingKey(), paid.processingKey());
        assertEquals(created.processingKey(), captured.processingKey());
        assertEquals(
            created.shipmentCreated(MAPPER, "shipment-1", "label_created").eventKey(),
            paid.shipmentCreated(MAPPER, "shipment-1", "label_created").eventKey()
        );
        assertEquals(
            created.notificationEffectKey("label_created"),
            paid.notificationEffectKey("label_created")
        );
        assertEquals(
            created.notificationEffectKey("label_created"),
            captured.notificationEffectKey("label_created")
        );
    }

    @Test
    void payment_events_require_the_payment_identity_and_settlement_fields() {
        String missingPaymentId = paymentJson("payment.failed", "failed")
            .replace("\"payment_id\":\"payment-1\",", "");
        String mismatchedPaymentId = paymentJson("payment.paid", "paid")
            .replace("\"payment_id\":\"payment-1\"", "\"payment_id\":\"payment-2\"");
        String missingAmount = paymentJson("payment.captured", "captured")
            .replace("\"amount_minor\":9177,", "");
        String missingCurrency = paymentJson("payment.captured", "captured")
            .replace("\"currency\":\"USD\",", "");
        String missingStatus = paymentJson("payment.captured", "captured")
            .replace("\"payment_status\":\"captured\"", "");

        assertThrows(PermanentEventException.class, () -> parse(missingPaymentId));
        assertThrows(PermanentEventException.class, () -> parse(mismatchedPaymentId));
        assertThrows(PermanentEventException.class, () -> parse(missingAmount));
        assertThrows(PermanentEventException.class, () -> parse(missingCurrency));
        assertThrows(PermanentEventException.class, () -> parse(missingStatus));

        CommerceEvent event = parse(paymentJson("payment.paid", "paid"));
        assertEquals("payment-1", event.paymentId());
        assertEquals("order-1", event.orderId());
        assertEquals(9177, event.totalMinor());
    }

    private static CommerceEvent parse(String json) {
        return CommerceEvent.parse(MAPPER, json.getBytes(StandardCharsets.UTF_8));
    }

    private static String validJson() {
        return """
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
            """;
    }

    private static String paymentJson(String eventType, String paymentStatus) {
        return ("""
            {
              "event_id":"event-payment-1",
              "event_key":"%s:payment-1",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"%s",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"payment",
              "aggregate_id":"payment-1",
              "payload":{
                "order_id":"order-1",
                "payment_id":"payment-1",
                "amount_minor":9177,
                "currency":"USD",
                "payment_status":"%s"
              }
            }
            """).formatted(eventType, eventType, paymentStatus);
    }
}
