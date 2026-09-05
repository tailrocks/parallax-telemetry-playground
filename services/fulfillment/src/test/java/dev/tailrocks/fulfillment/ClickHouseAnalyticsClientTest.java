package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.springframework.test.web.client.response.MockRestResponseCreators.withSuccess;

import com.fasterxml.jackson.databind.ObjectMapper;
import java.nio.charset.StandardCharsets;
import java.util.Base64;
import org.junit.jupiter.api.Test;
import org.springframework.http.HttpHeaders;
import org.springframework.test.web.client.MockRestServiceServer;
import org.springframework.web.client.RestClient;

class ClickHouseAnalyticsClientTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void sends_configured_clickhouse_basic_auth() {
        RestClient.Builder builder = RestClient.builder().baseUrl("http://clickhouse.test");
        MockRestServiceServer server = MockRestServiceServer.bindTo(builder).build();
        ClickHouseAnalyticsClient client = new ClickHouseAnalyticsClient(
            builder.build(),
            MAPPER,
            "analytics-user",
            "analytics-secret"
        );
        String expected = "Basic " + Base64.getEncoder().encodeToString(
            "analytics-user:analytics-secret".getBytes(StandardCharsets.UTF_8)
        );
        server.expect(request -> assertEquals(
            expected,
            request.getHeaders().getFirst(HttpHeaders.AUTHORIZATION)
        )).andRespond(withSuccess());

        client.ingest(event());

        server.verify();
    }

    @Test
    void rejects_a_password_without_a_clickhouse_username() {
        RestClient http = RestClient.builder().baseUrl("http://clickhouse.test").build();

        org.junit.jupiter.api.Assertions.assertThrows(
            IllegalArgumentException.class,
            () -> new ClickHouseAnalyticsClient(http, MAPPER, "", "secret")
        );
    }

    private static CommerceEvent event() {
        return CommerceEvent.parse(MAPPER, """
            {
              "event_id":"event-analytics-1",
              "event_key":"order.paid:order-analytics-1",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.paid",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-analytics-1",
              "payload":{"order_id":"order-analytics-1"}
            }
            """.getBytes(StandardCharsets.UTF_8));
    }
}
