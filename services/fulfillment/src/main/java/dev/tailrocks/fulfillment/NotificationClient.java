package dev.tailrocks.fulfillment;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.node.ObjectNode;
import io.opentelemetry.context.Context;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.http.HttpHeaders;
import org.springframework.http.MediaType;
import org.springframework.stereotype.Component;
import org.springframework.web.client.RestClient;

@Component
final class NotificationClient {
    static final String DURABLE_CHANNEL = "in_app";

    private final RestClient http;
    private final String notificationsUrl;
    private final ObjectMapper mapper;

    @Autowired
    NotificationClient(
        RestClient.Builder builder,
        @Value("${notifications.url:http://notifications:8091}") String notificationsUrl,
        ObjectMapper mapper
    ) {
        this(builder.clone().build(), notificationsUrl, mapper);
    }

    NotificationClient(RestClient http, String notificationsUrl, ObjectMapper mapper) {
        this.http = http;
        this.notificationsUrl = trimTrailingSlash(notificationsUrl);
        this.mapper = mapper;
    }

    void notifyOrder(CommerceEvent event, String fulfillmentStatus, Context context) {
        String idempotencyKey = event.notificationEffectKey(fulfillmentStatus);
        ObjectNode request = mapper.createObjectNode();
        request.put("tenant_id", event.tenantId());
        request.put("event_key", idempotencyKey);
        request.put("order_id", event.orderId());
        request.put("channel", DURABLE_CHANNEL);
        ObjectNode payload = request.putObject("payload");
        payload.put("event_type", event.eventType());
        payload.put("fulfillment_status", fulfillmentStatus);
        payload.put("customer_id", event.customerId() == null ? "" : event.customerId());

        HttpHeaders propagated = new HttpHeaders();
        RabbitTraceContext.inject(context, propagated);
        propagated.set(
            "Idempotency-Key",
            idempotencyKey
        );
        String body;
        try {
            body = mapper.writeValueAsString(request);
        } catch (JsonProcessingException error) {
            throw new IllegalStateException("failed to encode notification request", error);
        }

        http.post()
            .uri(notificationsUrl + "/notify")
            .headers(headers -> headers.putAll(propagated))
            .contentType(MediaType.APPLICATION_JSON)
            .body(body)
            .retrieve()
            .toBodilessEntity();
    }

    private static String trimTrailingSlash(String value) {
        if (value == null || value.isBlank()) {
            return "http://notifications:8091";
        }
        return value.endsWith("/") ? value.substring(0, value.length() - 1) : value;
    }
}
