package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.springframework.test.web.client.match.MockRestRequestMatchers.content;
import static org.springframework.test.web.client.response.MockRestResponseCreators.withSuccess;

import com.fasterxml.jackson.databind.ObjectMapper;
import io.opentelemetry.api.baggage.Baggage;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.SpanContext;
import io.opentelemetry.api.trace.TraceFlags;
import io.opentelemetry.api.trace.TraceState;
import io.opentelemetry.context.Context;
import java.nio.charset.StandardCharsets;
import org.junit.jupiter.api.Test;
import org.springframework.test.web.client.MockRestServiceServer;
import org.springframework.web.client.RestClient;

class NotificationClientTest {
    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void selects_the_explicit_in_app_durable_channel_and_preserves_w3c_headers() throws Exception {
        RestClient.Builder builder = RestClient.builder().baseUrl("http://notifications.test");
        MockRestServiceServer server = MockRestServiceServer.bindTo(builder).build();
        NotificationClient client = new NotificationClient(
            builder.build(),
            "http://notifications.test",
            MAPPER
        );
        server.expect(request -> {
            assertEquals(
                "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
                request.getHeaders().getFirst("traceparent")
            );
            assertEquals("vendor=state", request.getHeaders().getFirst("tracestate"));
            assertEquals("tenant.id=tenant-acme", request.getHeaders().getFirst("baggage"));
        }).andExpect(content().json("{\"channel\":\"in_app\"}", false))
            .andRespond(withSuccess());

        client.notifyOrder(event(), "label_created", validContext());

        server.verify();
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

    private static CommerceEvent event() {
        return CommerceEvent.parse(MAPPER, """
            {
              "event_id":"event-notification-1",
              "event_key":"order.created:order-notification-1",
              "schema_version":1,
              "tenant_id":"tenant-acme",
              "event_type":"order.created",
              "occurred_at":"2026-01-26T08:21:02Z",
              "aggregate_type":"order",
              "aggregate_id":"order-notification-1",
              "payload":{"order_id":"order-notification-1"}
            }
            """.getBytes(StandardCharsets.UTF_8));
    }
}
