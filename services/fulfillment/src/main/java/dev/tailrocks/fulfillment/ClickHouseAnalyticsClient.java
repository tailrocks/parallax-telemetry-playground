package dev.tailrocks.fulfillment;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import io.opentelemetry.api.baggage.Baggage;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.context.Context;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.nio.charset.StandardCharsets;
import java.time.ZoneOffset;
import java.time.format.DateTimeFormatter;
import java.util.UUID;
import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.http.HttpHeaders;
import org.springframework.http.MediaType;
import org.springframework.stereotype.Component;
import org.springframework.web.client.RestClient;

@Component
final class ClickHouseAnalyticsClient {
    private static final DateTimeFormatter CLICKHOUSE_TIMESTAMP =
        DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss.SSS").withZone(ZoneOffset.UTC);

    private static final String INSERT_QUERY = """
        INSERT INTO analytics.analytics_events
        (event_id, tenant_id, event_key, customer_id, session_id, event_name, event_version,
         source, entity_type, entity_id, occurred_at, trace_id, span_id, traceparent,
         tracestate, baggage, feature_variant, properties, context)
        FORMAT JSONEachRow
        """;

    private static final String EXPOSURE_INSERT_QUERY = """
        INSERT INTO analytics.feature_exposures
        (exposure_id, tenant_id, exposure_key, customer_id, anonymous_id, session_id,
         feature_key, variant, exposed_at, context, trace_id, span_id)
        FORMAT JSONEachRow
        """;

    private final RestClient http;
    private final ObjectMapper mapper;
    private final String clickhouseUser;
    private final String clickhousePassword;

    @Autowired
    ClickHouseAnalyticsClient(
        RestClient.Builder builder,
        @Value("${analytics.clickhouse-url:http://clickhouse:8123}") String clickhouseUrl,
        @Value("${analytics.clickhouse-user:default}") String clickhouseUser,
        @Value("${analytics.clickhouse-password:}") String clickhousePassword,
        ObjectMapper mapper
    ) {
        this(
            builder.clone().baseUrl(clickhouseUrl).build(),
            mapper,
            clickhouseUser,
            clickhousePassword
        );
    }

    ClickHouseAnalyticsClient(RestClient http, ObjectMapper mapper) {
        this(http, mapper, "", "");
    }

    ClickHouseAnalyticsClient(
        RestClient http,
        ObjectMapper mapper,
        String clickhouseUser,
        String clickhousePassword
    ) {
        this.http = http;
        this.mapper = mapper;
        this.clickhouseUser = clickhouseUser == null ? "" : clickhouseUser;
        this.clickhousePassword = clickhousePassword == null ? "" : clickhousePassword;
        if (this.clickhouseUser.isBlank() && !this.clickhousePassword.isBlank()) {
            throw new IllegalArgumentException(
                "analytics.clickhouse-user is required when a ClickHouse password is configured"
            );
        }
    }

    void ingest(CommerceEvent event) {
        Propagation propagation = propagation();
        var spanContext = Span.current().getSpanContext();
        String traceId = spanContext.isValid() ? spanContext.getTraceId() : "";
        String spanId = spanContext.isValid() ? spanContext.getSpanId() : "";
        ObjectNode row = mapper.createObjectNode();
        row.put("event_id", deterministicEventId(event.tenantId(), event.eventKey()));
        row.put("tenant_id", event.tenantId());
        row.put("event_key", event.eventKey());
        if (event.customerId() == null) {
            row.putNull("customer_id");
        } else {
            row.put("customer_id", event.customerId());
        }
        putNullable(row, "session_id", sessionId(event, propagation));
        row.put("event_name", event.eventType());
        row.put("event_version", event.schemaVersion());
        row.put("source", "fulfillment");
        row.put("entity_type", event.entityType());
        row.put("entity_id", event.entityId());
        row.put("occurred_at", CLICKHOUSE_TIMESTAMP.format(event.occurredAt()));
        row.put("trace_id", traceId);
        row.put("span_id", spanId);
        row.put("traceparent", propagation.traceparent());
        row.put("tracestate", propagation.tracestate());
        row.put("baggage", propagation.baggage());
        row.put("feature_variant", featureVariant(event, propagation));
        row.put("properties", event.payload().toString());
        row.put("context", context(event, propagation).toString());

        insert(INSERT_QUERY, row);
        ingestFeatureExposure(event, propagation);
    }

    private void ingestFeatureExposure(CommerceEvent event, Propagation propagation) {
        String variant = featureVariant(event, propagation);
        if (variant.isBlank()) {
            return;
        }
        String featureKey = firstText(event.payload(), "feature_key", "featureKey");
        if (featureKey.isBlank() && event.eventType().equals("order.paid")) {
            featureKey = "checkoutFlow";
        }
        if (featureKey.isBlank()) {
            return;
        }
        String exposureKey = firstText(event.payload(), "exposure_key", "exposureKey");
        if (exposureKey.isBlank()) {
            exposureKey = event.eventKey() + ":" + featureKey;
        }
        ObjectNode row = mapper.createObjectNode();
        row.put("exposure_id", deterministicEventId(event.tenantId(), "exposure:" + exposureKey));
        row.put("tenant_id", event.tenantId());
        row.put("exposure_key", exposureKey);
        putNullable(row, "customer_id", event.customerId());
        putNullable(row, "anonymous_id", firstText(event.payload(), "anonymous_id", "anonymousId"));
        putNullable(row, "session_id", sessionId(event, propagation));
        row.put("feature_key", featureKey);
        row.put("variant", variant);
        row.put("exposed_at", CLICKHOUSE_TIMESTAMP.format(event.occurredAt()));
        row.put("context", context(event, propagation).toString());
        row.put("trace_id", propagation.traceId());
        row.put("span_id", propagation.spanId());
        insert(EXPOSURE_INSERT_QUERY, row);
    }

    private void insert(String query, ObjectNode row) {
        final String body;
        try {
            body = mapper.writeValueAsString(row) + "\n";
        } catch (JsonProcessingException error) {
            throw new IllegalStateException("failed to encode ClickHouse analytics row", error);
        }
        http.post()
            .uri(uri -> uri.path("/").queryParam("query", query).build())
            .headers(headers -> {
                if (!clickhouseUser.isBlank() || !clickhousePassword.isBlank()) {
                    headers.setBasicAuth(
                        clickhouseUser,
                        clickhousePassword,
                        StandardCharsets.UTF_8
                    );
                }
            })
            .contentType(MediaType.APPLICATION_JSON)
            .body(body)
            .retrieve()
            .toBodilessEntity();
    }

    private Propagation propagation() {
        HttpHeaders headers = new HttpHeaders();
        RabbitTraceContext.inject(Context.current(), headers);
        var spanContext = Span.current().getSpanContext();
        return new Propagation(
            header(headers, "traceparent"),
            header(headers, "tracestate"),
            header(headers, "baggage"),
            spanContext.isValid() ? spanContext.getTraceId() : "",
            spanContext.isValid() ? spanContext.getSpanId() : "",
            Baggage.fromContext(Context.current())
        );
    }

    private static String header(HttpHeaders headers, String name) {
        String value = headers.getFirst(name);
        return value == null ? "" : value;
    }

    private ObjectNode context(CommerceEvent event, Propagation propagation) {
        ObjectNode result = mapper.createObjectNode();
        JsonNode eventContext = event.context();
        if (eventContext != null && eventContext.isObject()) {
            result.set("attributes", eventContext.deepCopy());
        }
        result.put("traceparent", propagation.traceparent());
        result.put("tracestate", propagation.tracestate());
        result.put("baggage", propagation.baggage());
        ObjectNode baggageItems = mapper.createObjectNode();
        propagation.baggageContext().forEach((key, entry) ->
            baggageItems.put(key, entry.getValue())
        );
        result.set("baggage_items", baggageItems);
        result.put("feature_variant", featureVariant(event, propagation));
        return result;
    }

    private String featureVariant(CommerceEvent event, Propagation propagation) {
        String eventVariant = firstText(event.payload(), "feature_variant", "featureVariant");
        if (!eventVariant.isBlank()) {
            return eventVariant;
        }
        String baggageVariant = propagation.baggageContext().getEntryValue("feature.variant");
        return baggageVariant == null ? "" : baggageVariant;
    }

    private String sessionId(CommerceEvent event, Propagation propagation) {
        String eventSession = event.sessionId();
        if (eventSession != null && !eventSession.isBlank()) {
            return eventSession;
        }
        return propagation.baggageContext().getEntryValue("session.id");
    }

    private static String firstText(JsonNode node, String... names) {
        if (node == null) {
            return "";
        }
        for (String name : names) {
            JsonNode value = node.get(name);
            if (value != null && !value.isNull()) {
                String text = value.asText("").trim();
                if (!text.isBlank()) {
                    return text;
                }
            }
        }
        JsonNode context = node.get("context");
        if (context != null && context.isObject()) {
            for (String name : names) {
                JsonNode value = context.get(name);
                if (value != null && !value.isNull()) {
                    String text = value.asText("").trim();
                    if (!text.isBlank()) {
                        return text;
                    }
                }
            }
        }
        return "";
    }

    private static void putNullable(ObjectNode row, String name, String value) {
        if (value == null || value.isBlank()) {
            row.putNull(name);
        } else {
            row.put(name, value);
        }
    }

    static String deterministicEventId(String tenantId, String eventKey) {
        byte[] identity = ("parallax.analytics.v1\0" + tenantId + "\0" + eventKey)
            .getBytes(StandardCharsets.UTF_8);
        final byte[] digest;
        try {
            digest = MessageDigest.getInstance("SHA-256").digest(identity);
        } catch (NoSuchAlgorithmException error) {
            throw new IllegalStateException("SHA-256 is unavailable", error);
        }
        long mostSignificantBits = 0;
        long leastSignificantBits = 0;
        for (int index = 0; index < 8; index++) {
            mostSignificantBits = (mostSignificantBits << 8) | (digest[index] & 0xffL);
            leastSignificantBits =
                (leastSignificantBits << 8) | (digest[index + 8] & 0xffL);
        }
        mostSignificantBits = (mostSignificantBits & 0xffffffffffff0fffL) | 0x5000L;
        leastSignificantBits =
            (leastSignificantBits & 0x3fffffffffffffffL) | 0x8000000000000000L;
        return new UUID(mostSignificantBits, leastSignificantBits).toString();
    }

    private record Propagation(
        String traceparent,
        String tracestate,
        String baggage,
        String traceId,
        String spanId,
        Baggage baggageContext
    ) {}
}
