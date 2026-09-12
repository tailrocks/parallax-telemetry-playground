package dev.tailrocks.fulfillment;

import org.springframework.amqp.core.MessageProperties;

/**
 * Validates the broker route independently from the caller-controlled event body.
 * Retry messages carry the original commerce-events route in immutable headers.
 */
record RabbitDeliveryRoute(
    String receivedExchange,
    String receivedRoutingKey,
    String originalExchange,
    String originalRoutingKey
) {
    static final String ORIGINAL_EXCHANGE_HEADER = "x-commerce-original-exchange";
    static final String ORIGINAL_ROUTING_KEY_HEADER = "x-commerce-original-routing-key";

    static RabbitDeliveryRoute from(MessageProperties properties) {
        if (properties == null) {
            throw new PermanentEventException("RabbitMQ message properties are required");
        }
        return new RabbitDeliveryRoute(
            clean(properties.getReceivedExchange()),
            clean(properties.getReceivedRoutingKey()),
            clean(properties.getHeaders().get(ORIGINAL_EXCHANGE_HEADER)),
            clean(properties.getHeaders().get(ORIGINAL_ROUTING_KEY_HEADER))
        );
    }

    static void preserveOriginalRoute(MessageProperties source, MessageProperties target) {
        RabbitDeliveryRoute route = from(source);
        String exchange = route.originalExchange().isBlank()
            ? route.receivedExchange()
            : route.originalExchange();
        String routingKey = route.originalRoutingKey().isBlank()
            ? route.receivedRoutingKey()
            : route.originalRoutingKey();
        if (exchange.isBlank() || routingKey.isBlank()) {
            throw new RetryableEventException(
                "cannot defer RabbitMQ message without its original route"
            );
        }
        target.setHeader(ORIGINAL_EXCHANGE_HEADER, exchange);
        target.setHeader(ORIGINAL_ROUTING_KEY_HEADER, routingKey);
    }

    void validate(CommerceEvent event) {
        validateRetryRoute(event, RabbitMessagingConfiguration.ORDERS_RETRY_ROUTING_KEY);
    }

    void validateForAnalytics(CommerceEvent event, String analyticsQueueName) {
        validateRetryRoute(
            event,
            RabbitMessagingConfiguration.analyticsRetryRoutingKey(analyticsQueueName)
        );
    }

    private void validateRetryRoute(CommerceEvent event, String retryRoutingKey) {
        if (receivedExchange().equals(RabbitMessagingConfiguration.EVENTS_EXCHANGE)) {
            if (!originalExchange().isBlank() || !originalRoutingKey().isBlank()) {
                throw mismatch("initial commerce event must not carry retry route metadata");
            }
            requireRoute(event.eventType());
            return;
        }
        if (receivedExchange().equals(RabbitMessagingConfiguration.RETRY_RETURN_EXCHANGE)
            && receivedRoutingKey().equals(retryRoutingKey)) {
            if (!originalExchange().equals(RabbitMessagingConfiguration.EVENTS_EXCHANGE)
                || !originalRoutingKey().equals(event.eventType())) {
                throw mismatch("retry original route does not match event_type");
            }
            return;
        }
        throw mismatch(
            "unexpected fulfillment route exchange=" + receivedExchange()
                + " routing_key=" + receivedRoutingKey()
        );
    }

    private void requireRoute(String eventType) {
        if (receivedRoutingKey().isBlank() || !receivedRoutingKey().equals(eventType)) {
            throw mismatch("RabbitMQ routing key does not match event_type");
        }
    }

    private static PermanentEventException mismatch(String message) {
        return new PermanentEventException(message);
    }

    private static String clean(Object value) {
        if (value instanceof byte[] bytes) {
            return new String(bytes, java.nio.charset.StandardCharsets.UTF_8).trim();
        }
        return value == null ? "" : value.toString().trim();
    }
}
