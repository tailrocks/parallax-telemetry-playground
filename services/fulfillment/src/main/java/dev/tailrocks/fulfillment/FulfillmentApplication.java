package dev.tailrocks.fulfillment;

import dev.tailrocks.pricing.v1.PricingGrpc;
import dev.tailrocks.pricing.v1.QuoteRequest;
import io.opentelemetry.api.common.AttributeKey;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.SpanContext;
import io.opentelemetry.api.trace.propagation.W3CTraceContextPropagator;
import io.opentelemetry.context.Context;
import io.opentelemetry.context.propagation.TextMapGetter;
import io.opentelemetry.context.propagation.TextMapSetter;
import io.tailrocks.semconv.Semconv;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.UUID;
import org.apache.kafka.clients.consumer.ConsumerRecord;
import org.apache.kafka.clients.producer.ProducerRecord;
import org.apache.kafka.common.header.Header;
import org.apache.kafka.common.header.Headers;
import org.springframework.context.annotation.Bean;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.grpc.client.GrpcChannelFactory;
import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.kafka.core.KafkaTemplate;
import org.springframework.stereotype.Component;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.client.RestClient;

@SpringBootApplication
public class FulfillmentApplication {
    public static void main(String[] args) {
        SpringApplication.run(FulfillmentApplication.class, args);
    }

    @Bean
    PricingGrpc.PricingBlockingStub paymentPricingClient(GrpcChannelFactory channels) {
        return PricingGrpc.newBlockingStub(channels.createChannel("payment"));
    }
}

// Real-Kafka producer: POST /publish sends to the `orders` topic (PRODUCER span,
// agent-instrumented), which the consumer below picks up over the broker
// (CONSUMER span). Replaces the in-process queue with a real broker round-trip.
@RestController
class OrderProducer {
    private final KafkaTemplate<String, String> kafka;

    OrderProducer(KafkaTemplate<String, String> kafka) {
        this.kafka = kafka;
    }

    static final String JOB_ID_HEADER = "x-job-id";

    @PostMapping("/publish")
    String publish(@RequestParam(defaultValue = "order-1") String order) {
        // Detached-job identity (plan 158 decision 6): the PRODUCER mints a
        // job id, stamps it on its span, and carries it over Kafka headers so
        // the CONSUMER attempt shares the same job.
        String jobId = UUID.randomUUID().toString();
        Span.current().setAttribute(AttributeKey.stringKey(Semconv.JOB_ID), jobId);
        Span.current()
            .setAttribute(
                AttributeKey.stringKey(Semconv.JOB_TYPE), Semconv.JOB_TYPE_FULFILLMENT_SHIPMENT);
        ProducerRecord<String, String> record = new ProducerRecord<>("orders", order);
        KafkaTraceContext.inject(Context.current(), record.headers());
        record.headers().add(JOB_ID_HEADER, jobId.getBytes(StandardCharsets.US_ASCII));
        kafka.send(record);
        return "published " + order;
    }
}

@Component
class NotificationClient {
    private final RestClient http;
    private final String notificationsUrl =
        System.getenv().getOrDefault("NOTIFICATIONS_URL", "http://notifications:8091");

    NotificationClient() {
        this(RestClient.create());
    }

    NotificationClient(RestClient http) {
        this.http = http;
    }

    void notifyOrder() {
        http.get().uri(notificationsUrl + "/").retrieve().toBodilessEntity();
    }
}

@Component
class OrderConsumer {
    private final PricingGrpc.PricingBlockingStub pricing;
    private final NotificationClient notifications;

    OrderConsumer(PricingGrpc.PricingBlockingStub pricing, NotificationClient notifications) {
        this.pricing = pricing;
        this.notifications = notifications;
    }

    // CONSUMER span (auto-instrumented); the reverse Java→Rust hop follows.
    @KafkaListener(topics = "orders", groupId = "fulfillment")
    void onOrder(ConsumerRecord<String, String> record) {
        Context producer = KafkaTraceContext.extract(record.headers());
        SpanContext producerSpan = Span.fromContext(producer).getSpanContext();
        if (producerSpan.isValid()) {
            Span.current().addLink(producerSpan);
        }
        String jobId = KafkaTraceContext.header(record.headers(), OrderProducer.JOB_ID_HEADER);
        if (jobId != null && !jobId.isEmpty()) {
            Span.current().setAttribute(AttributeKey.stringKey(Semconv.JOB_ID), jobId);
            Span.current()
                .setAttribute(
                    AttributeKey.stringKey(Semconv.JOB_TYPE),
                    Semconv.JOB_TYPE_FULFILLMENT_SHIPMENT);
        }
        String order = record.value();
        pricing.quote(QuoteRequest.newBuilder().setSku(order).setQuantity(1).build());
        notifications.notifyOrder();
    }
}

final class KafkaTraceContext {
    private static final W3CTraceContextPropagator W3C = W3CTraceContextPropagator.getInstance();
    private static final TextMapSetter<Headers> SETTER = (headers, key, value) -> {
        headers.remove(key);
        headers.add(key, value.getBytes(StandardCharsets.US_ASCII));
    };
    private static final TextMapGetter<Headers> GETTER = new TextMapGetter<>() {
        @Override
        public Iterable<String> keys(Headers headers) {
            List<String> keys = new ArrayList<>();
            for (Header header : headers) {
                keys.add(header.key());
            }
            return keys;
        }

        @Override
        public String get(Headers headers, String key) {
            Header header = headers.lastHeader(key);
            return header == null ? null : new String(header.value(), StandardCharsets.US_ASCII);
        }
    };

    private KafkaTraceContext() {}

    static void inject(Context context, Headers headers) {
        W3C.inject(context, headers, SETTER);
    }

    static Context extract(Headers headers) {
        return W3C.extract(Context.root(), headers, GETTER);
    }

    static String header(Headers headers, String key) {
        Header header = headers.lastHeader(key);
        return header == null ? null : new String(header.value(), StandardCharsets.US_ASCII);
    }
}
