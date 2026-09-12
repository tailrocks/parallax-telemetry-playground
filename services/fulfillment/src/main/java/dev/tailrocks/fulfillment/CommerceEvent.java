package dev.tailrocks.fulfillment;

import com.fasterxml.jackson.core.JsonParser;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.JsonNodeFactory;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.Map;
import java.util.Set;

/** Versioned commerce envelope shared by the RabbitMQ order and analytics queues. */
record CommerceEvent(
    String eventId,
    String eventKey,
    int schemaVersion,
    String tenantId,
    String eventType,
    Instant occurredAt,
    String orderId,
    String customerId,
    String currency,
    long totalMinor,
    String paymentStatus,
    JsonNode payload,
    String entityType,
    String entityId,
    JsonNode context,
    String sessionId
) {
    static final int CURRENT_SCHEMA_VERSION = 1;

    private static final Set<String> ENVELOPE_FIELDS = Set.of(
        "event_id",
        "event_key",
        "schema_version",
        "tenant_id",
        "event_type",
        "occurred_at",
        "aggregate_type",
        "aggregate_id",
        "entity_type",
        "entity_id",
        "session_id",
        "context",
        "payload"
    );

    private static final Set<String> ORDER_AGGREGATE_EVENT_TYPES = Set.of(
        "order.created",
        "order.confirmed",
        "order.paid",
        "order.cancelled"
    );

    private static final Set<String> PAYMENT_EVENT_TYPES = Set.of(
        "payment.authorized",
        "payment.captured",
        "payment.paid",
        "payment.failed"
    );

    CommerceEvent(
        String eventId,
        String eventKey,
        int schemaVersion,
        String tenantId,
        String eventType,
        Instant occurredAt,
        String orderId,
        String customerId,
        String currency,
        long totalMinor,
        String paymentStatus,
        JsonNode payload
    ) {
        this(
            eventId,
            eventKey,
            schemaVersion,
            tenantId,
            eventType,
            occurredAt,
            orderId,
            customerId,
            currency,
            totalMinor,
            paymentStatus,
            payload,
            inferredEntityType(payload, orderId),
            inferredEntityId(payload, orderId, eventKey),
            inferredContext(payload),
            inferredSessionId(payload)
        );
    }

    static CommerceEvent parse(ObjectMapper mapper, byte[] body) {
        return parse(mapper, body, Map.of(), null);
    }

    static CommerceEvent parse(
        ObjectMapper mapper,
        byte[] body,
        Map<String, Object> headers,
        String messageId
    ) {
        if (body == null) {
            throw new PermanentEventException("commerce event body is required");
        }
        try {
            JsonNode root;
            try (JsonParser parser = mapper.getFactory().createParser(body)) {
                parser.enable(JsonParser.Feature.STRICT_DUPLICATE_DETECTION);
                root = mapper.readTree(parser);
                if (parser.nextToken() != null) {
                    throw new PermanentEventException("commerce event contains multiple JSON values");
                }
            }
            if (root == null || !root.isObject()) {
                throw new PermanentEventException("commerce event must be a JSON object");
            }
            CommerceEvent event = fromRoot(root);
            validateHeaders(event, headers, messageId);
            return event;
        } catch (IOException error) {
            throw new PermanentEventException("commerce event is not valid JSON", error);
        }
    }

    private static CommerceEvent fromRoot(JsonNode root) {
        root.fieldNames().forEachRemaining(field -> {
            if (!ENVELOPE_FIELDS.contains(field)) {
                throw new PermanentEventException("commerce event has unknown envelope field: " + field);
            }
        });

        JsonNode payload = root.get("payload");
        if (payload == null || !payload.isObject()) {
            throw new PermanentEventException("commerce event payload must be a JSON object");
        }

        String eventId = requiredText(root, "event_id");
        String eventKey = requiredText(root, "event_key");
        int schemaVersion = requiredInt(root, "schema_version");
        if (schemaVersion != CURRENT_SCHEMA_VERSION) {
            throw new PermanentEventException(
                "unsupported commerce event schema_version: " + schemaVersion
            );
        }

        String tenantId = requiredText(root, "tenant_id");
        String eventType = requiredText(root, "event_type");
        Instant occurredAt = requiredInstant(root, "occurred_at");
        String aggregateType = requiredText(root, "aggregate_type");
        String aggregateId = requiredText(root, "aggregate_id");

        String entityType = optionalText(root, "entity_type");
        String entityId = optionalText(root, "entity_id");
        if ((entityType == null) != (entityId == null)) {
            throw new PermanentEventException(
                "commerce event entity_type and entity_id must be supplied together"
            );
        }
        if (entityType == null) {
            entityType = aggregateType;
            entityId = aggregateId;
        } else if (!entityType.equals(aggregateType) || !entityId.equals(aggregateId)) {
            throw new PermanentEventException(
                "commerce event entity identity does not match aggregate identity"
            );
        }

        String payloadOrderId = optionalText(payload, "order_id");
        String orderId;
        if (ORDER_AGGREGATE_EVENT_TYPES.contains(eventType)) {
            if (!aggregateType.equals("order")) {
                throw new PermanentEventException(
                    "order event must use an order aggregate: " + eventType
                );
            }
            // The order aggregate is authoritative for order.created events;
            // older durable outbox rows do not repeat it inside payload.
            orderId = aggregateId;
            if (payloadOrderId != null && payloadOrderId.isBlank()) {
                throw new PermanentEventException(
                    "commerce event payload.order_id must not be blank"
                );
            }
            if (payloadOrderId != null && !aggregateId.equals(payloadOrderId)) {
                throw new PermanentEventException(
                    "commerce event aggregate_id does not match payload.order_id"
                );
            }
        } else if (PAYMENT_EVENT_TYPES.contains(eventType)) {
            if (!aggregateType.equals("payment")) {
                throw new PermanentEventException(
                    "payment event must use a payment aggregate: " + eventType
                );
            }
            if (payloadOrderId == null || payloadOrderId.isBlank()) {
                throw new PermanentEventException(
                    "commerce event is missing payload.order_id for " + eventType
                );
            }
            String paymentId = requiredText(payload, "payment_id");
            if (!aggregateId.equals(paymentId)) {
                throw new PermanentEventException(
                    "commerce event aggregate_id does not match payload.payment_id"
                );
            }
            orderId = payloadOrderId;
        } else {
            orderId = payloadOrderId == null ? "" : payloadOrderId;
            if (aggregateType.equals("order")
                && (orderId.isBlank() || !aggregateId.equals(orderId))) {
                throw new PermanentEventException(
                    "commerce event aggregate_id does not match payload.order_id"
                );
            }
        }

        JsonNode contextNode = root.get("context");
        if (contextNode != null && !contextNode.isObject()) {
            throw new PermanentEventException("commerce event context must be a JSON object");
        }
        JsonNode context = contextNode == null
            ? JsonNodeFactory.instance.objectNode()
            : contextNode.deepCopy();
        String sessionId = optionalText(root, "session_id");
        String customerId = optionalText(payload, "customer_id");
        String currency = optionalText(payload, "currency");
        String paymentStatus = optionalText(payload, "payment_status");
        JsonNode payment = object(payload, "payment");
        if (payment != null && paymentStatus == null) {
            paymentStatus = optionalText(payment, "status");
        }
        long totalMinor = PAYMENT_EVENT_TYPES.contains(eventType)
            ? requiredLong(payload, "amount_minor", "total_minor")
            : optionalLong(payload, "total_minor", "amount_minor");

        if (PAYMENT_EVENT_TYPES.contains(eventType)) {
            if (currency == null || currency.isBlank()) {
                throw new PermanentEventException(
                    "commerce event is missing payload.currency for " + eventType
                );
            }
            if (paymentStatus == null || paymentStatus.isBlank()) {
                throw new PermanentEventException(
                    "commerce event is missing payload.payment_status for " + eventType
                );
            }
        }

        return new CommerceEvent(
            eventId,
            eventKey,
            schemaVersion,
            tenantId,
            eventType,
            occurredAt,
            orderId,
            customerId,
            currency,
            totalMinor,
            paymentStatus == null ? "" : paymentStatus,
            payload.deepCopy(),
            entityType,
            entityId,
            context,
            sessionId
        );
    }

    private static void validateHeaders(
        CommerceEvent event,
        Map<String, Object> headers,
        String messageId
    ) {
        Map<String, Object> safeHeaders = headers == null ? Map.of() : headers;
        String headerEventKey = headerText(safeHeaders, "event-key");
        String headerEventType = headerText(safeHeaders, "event-type");
        String headerTenantId = headerText(safeHeaders, "tenant-id");
        boolean hasIdentityHeaders = safeHeaders.containsKey("event-key")
            || safeHeaders.containsKey("event-type")
            || safeHeaders.containsKey("tenant-id");
        if (hasIdentityHeaders) {
            requireHeader(headerEventKey, "event-key");
            requireHeader(headerEventType, "event-type");
            requireHeader(headerTenantId, "tenant-id");
            assertIdentity("event-key", event.eventKey(), headerEventKey);
            assertIdentity("event-type", event.eventType(), headerEventType);
            assertIdentity("tenant-id", event.tenantId(), headerTenantId);
        }

        String headerMessageId = headerText(safeHeaders, "message-id");
        boolean hasMessageIdHeader = safeHeaders.containsKey("message-id");
        if (hasMessageIdHeader) {
            requireHeader(headerMessageId, "message-id");
            assertIdentity("message-id", event.eventId(), headerMessageId);
        }
        if (messageId != null) {
            String providedMessageId = messageId.trim();
            requireHeader(providedMessageId, "message-id");
            assertIdentity("message-id", event.eventId(), providedMessageId);
            if (hasMessageIdHeader) {
                assertIdentity("message-id", providedMessageId, headerMessageId);
            }
        }
    }

    private static void requireHeader(String value, String name) {
        if (value.isBlank()) {
            throw new PermanentEventException("commerce event header is missing " + name);
        }
    }

    private static void assertIdentity(String name, String bodyValue, String headerValue) {
        if (!bodyValue.equals(headerValue.trim())) {
            throw new PermanentEventException(
                "commerce event " + name + " does not match its RabbitMQ identity"
            );
        }
    }

    private static String headerText(Map<String, Object> headers, String name) {
        Object value = headers.get(name);
        if (value == null) {
            return "";
        }
        if (value instanceof byte[] bytes) {
            return new String(bytes, StandardCharsets.UTF_8).trim();
        }
        return value.toString().trim();
    }

    boolean isCancellation() {
        return !orderId.isBlank()
            && (eventType.equals("order.cancelled") || eventType.equals("payment.failed"));
    }

    boolean isPaymentEvent() {
        return PAYMENT_EVENT_TYPES.contains(eventType);
    }

    String paymentId() {
        return optionalText(payload, "payment_id");
    }

    boolean isFulfillmentTrigger() {
        return !orderId.isBlank()
            && (eventType.equals("order.created")
            || eventType.equals("order.confirmed")
            || eventType.equals("order.paid")
            || eventType.equals("payment.authorized")
            || eventType.equals("payment.captured")
            || eventType.equals("payment.paid"));
    }

    String fulfillmentEffectKey() {
        requireOrderId();
        return "fulfillment:" + tenantId + ":" + orderId;
    }

    String cancellationEffectKey() {
        requireOrderId();
        return "cancellation:" + tenantId + ":" + orderId;
    }

    String processingKey() {
        return isCancellation() ? cancellationEffectKey() : fulfillmentEffectKey();
    }

    String notificationEffectKey(String fulfillmentStatus) {
        String operationKey = isCancellation() ? cancellationEffectKey() : fulfillmentEffectKey();
        return operationKey + ":notification:" + fulfillmentStatus;
    }

    private void requireOrderId() {
        if (orderId == null || orderId.isBlank()) {
            throw new PermanentEventException("commerce event requires an order id");
        }
    }

    CommerceEvent shipmentCreated(ObjectMapper mapper, String shipmentId, String shipmentStatus) {
        ObjectNode result = mapper.createObjectNode();
        result.put("order_id", orderId);
        result.put("shipment_id", shipmentId);
        result.put("status", shipmentStatus);
        result.put("payment_status", paymentStatus);
        result.put("total_minor", totalMinor);
        if (sessionId != null && !sessionId.isBlank()) {
            result.put("session_id", sessionId);
        }
        return new CommerceEvent(
            fulfillmentEffectKey() + ":shipment",
            fulfillmentEffectKey() + ":shipment",
            CURRENT_SCHEMA_VERSION,
            tenantId,
            "fulfillment.shipment.created",
            Instant.now(),
            orderId,
            customerId,
            currency,
            totalMinor,
            paymentStatus,
            result
        );
    }

    CommerceEvent orderCancelled(ObjectMapper mapper) {
        ObjectNode result = mapper.createObjectNode();
        result.put("order_id", orderId);
        result.put("status", "cancelled");
        result.put("payment_status", paymentStatus);
        if (sessionId != null && !sessionId.isBlank()) {
            result.put("session_id", sessionId);
        }
        return new CommerceEvent(
            cancellationEffectKey() + ":event",
            cancellationEffectKey() + ":event",
            CURRENT_SCHEMA_VERSION,
            tenantId,
            "fulfillment.order.cancelled",
            Instant.now(),
            orderId,
            customerId,
            currency,
            totalMinor,
            paymentStatus,
            result
        );
    }

    String toJson(ObjectMapper mapper) {
        ObjectNode root = mapper.createObjectNode();
        root.put("event_id", eventId);
        root.put("event_key", eventKey);
        root.put("schema_version", schemaVersion);
        root.put("tenant_id", tenantId);
        root.put("event_type", eventType);
        root.put("occurred_at", occurredAt.toString());
        root.put("entity_type", entityType);
        root.put("entity_id", entityId);
        root.put("aggregate_type", entityType);
        root.put("aggregate_id", entityId);
        if (sessionId != null && !sessionId.isBlank()) {
            root.put("session_id", sessionId);
        }
        if (context != null && context.isObject() && context.size() > 0) {
            root.set("context", context.deepCopy());
        }
        root.set("payload", payload);
        try {
            return mapper.writeValueAsString(root);
        } catch (IOException error) {
            throw new IllegalStateException("failed to encode commerce event", error);
        }
    }

    private static JsonNode object(JsonNode node, String field) {
        if (node == null) {
            return null;
        }
        JsonNode value = node.get(field);
        return value != null && value.isObject() ? value : null;
    }

    private static String inferredEntityType(JsonNode payload, String orderId) {
        String entityType = text(payload, "entity_type");
        if (entityType.isBlank()) {
            entityType = text(payload, "aggregate_type");
        }
        return entityType.isBlank()
            ? (orderId == null || orderId.isBlank() ? "event" : "order")
            : entityType;
    }

    private static String inferredEntityId(JsonNode payload, String orderId, String eventKey) {
        String entityId = text(payload, "entity_id");
        if (entityId.isBlank()) {
            entityId = text(payload, "aggregate_id");
        }
        if (!entityId.isBlank()) {
            return entityId;
        }
        return orderId == null || orderId.isBlank() ? eventKey : orderId;
    }

    private static JsonNode inferredContext(JsonNode node) {
        return inferredContext(node, null);
    }

    private static String inferredSessionId(JsonNode node) {
        String value = text(node, "session_id");
        return value.isBlank() ? null : value;
    }

    private static JsonNode inferredContext(JsonNode first, JsonNode second) {
        ObjectNode result = JsonNodeFactory.instance.objectNode();
        copyContext(result, first);
        copyContext(result, second);
        return result;
    }

    private static void copyContext(ObjectNode target, JsonNode node) {
        JsonNode context = object(node, "context");
        if (context != null) {
            target.setAll((ObjectNode) context);
        }
    }

    private static String text(JsonNode node, String name) {
        if (node == null || node.get(name) == null || node.get(name).isNull()) {
            return "";
        }
        return node.get(name).asText("").trim();
    }

    private static String requiredText(JsonNode node, String field) {
        String value = optionalText(node, field);
        if (value == null || value.isBlank()) {
            throw new PermanentEventException("commerce event is missing " + field);
        }
        return value;
    }

    private static String optionalText(JsonNode node, String field) {
        if (node == null || node.get(field) == null) {
            return null;
        }
        JsonNode value = node.get(field);
        if (!value.isTextual()) {
            throw new PermanentEventException("commerce event field must be text: " + field);
        }
        return value.asText().trim();
    }

    private static int requiredInt(JsonNode node, String field) {
        JsonNode value = node.get(field);
        if (value == null || !value.isIntegralNumber() || !value.canConvertToInt()) {
            throw new PermanentEventException("commerce event field must be an integer: " + field);
        }
        return value.asInt();
    }

    private static long optionalLong(JsonNode node, String... fields) {
        for (String field : fields) {
            JsonNode value = node.get(field);
            if (value == null) {
                continue;
            }
            if (!value.isIntegralNumber() || !value.canConvertToLong()) {
                throw new PermanentEventException("commerce event field must be an integer: " + field);
            }
            return value.asLong();
        }
        return 0;
    }

    private static long requiredLong(JsonNode node, String... fields) {
        Long value = null;
        String firstField = null;
        for (String field : fields) {
            JsonNode candidate = node.get(field);
            if (candidate == null) {
                continue;
            }
            if (!candidate.isIntegralNumber() || !candidate.canConvertToLong()) {
                throw new PermanentEventException("commerce event field must be an integer: " + field);
            }
            long candidateValue = candidate.asLong();
            if (value != null && value != candidateValue) {
                throw new PermanentEventException(
                    "commerce event amount fields do not agree: " + firstField + ", " + field
                );
            }
            if (value == null) {
                value = candidateValue;
                firstField = field;
            }
        }
        if (value == null) {
            throw new PermanentEventException(
                "commerce event is missing one of " + String.join(", ", fields)
            );
        }
        return value;
    }

    private static Instant requiredInstant(JsonNode node, String field) {
        String value = requiredText(node, field);
        try {
            return Instant.parse(value);
        } catch (RuntimeException error) {
            throw new PermanentEventException("commerce event has invalid " + field, error);
        }
    }
}

class PermanentEventException extends RuntimeException {
    PermanentEventException(String message) {
        super(message);
    }

    PermanentEventException(String message, Throwable cause) {
        super(message, cause);
    }
}

class RetryableEventException extends RuntimeException {
    RetryableEventException(String message) {
        super(message);
    }

    RetryableEventException(String message, Throwable cause) {
        super(message, cause);
    }
}
