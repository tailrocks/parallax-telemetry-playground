package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;

import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.SpanContext;
import io.opentelemetry.api.trace.TraceFlags;
import io.opentelemetry.api.trace.TraceState;
import io.opentelemetry.context.Context;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import dev.tailrocks.pricing.v1.PricingGrpc;
import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.ManagedChannel;
import io.grpc.Server;
import io.grpc.inprocess.InProcessChannelBuilder;
import io.grpc.inprocess.InProcessServerBuilder;
import io.grpc.stub.StreamObserver;
import org.apache.kafka.clients.producer.ProducerRecord;
import org.apache.kafka.clients.consumer.Consumer;
import org.apache.kafka.common.serialization.StringDeserializer;
import org.apache.kafka.common.serialization.StringSerializer;
import org.apache.kafka.common.header.internals.RecordHeaders;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import org.springframework.kafka.core.DefaultKafkaProducerFactory;
import org.springframework.kafka.core.KafkaTemplate;
import org.springframework.kafka.test.EmbeddedKafkaBroker;
import org.springframework.kafka.test.condition.EmbeddedKafkaCondition;
import org.springframework.kafka.test.context.EmbeddedKafka;
import org.springframework.kafka.test.utils.KafkaTestUtils;
import org.mockito.ArgumentCaptor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

@ExtendWith(OpenTelemetryTestExtension.class)
@EmbeddedKafka(partitions = 1, topics = "orders")
class OrderProducerTest {
    private static EmbeddedKafkaBroker broker;

    @BeforeAll
    static void startBroker() {
        broker = EmbeddedKafkaCondition.getBroker();
    }

    @AfterAll
    static void stopBroker() {
        broker.destroy();
    }

    @Test
    void publishes_the_requested_order_to_the_orders_topic() {
        @SuppressWarnings("unchecked")
        KafkaTemplate<String, String> kafka = mock(KafkaTemplate.class);
        OrderProducer producer = new OrderProducer(kafka);

        assertEquals("published order-42", producer.publish("order-42"));
        @SuppressWarnings({"rawtypes", "unchecked"})
        ArgumentCaptor<ProducerRecord<String, String>> record = (ArgumentCaptor)
            ArgumentCaptor.forClass(ProducerRecord.class);
        verify(kafka).send(record.capture());
        assertEquals("orders", record.getValue().topic());
        assertEquals("order-42", record.getValue().value());
    }

    @Test
    void round_trips_a_w3c_producer_context_through_kafka_headers() {
        RecordHeaders headers = new RecordHeaders();
        SpanContext span = SpanContext.createFromRemoteParent(
            "0af7651916cd43dd8448eb211c80319c",
            "b7ad6b7169203331",
            TraceFlags.getSampled(),
            TraceState.getDefault()
        );

        KafkaTraceContext.inject(Context.root().with(Span.wrap(span)), headers);
        SpanContext extracted = Span.fromContext(KafkaTraceContext.extract(headers)).getSpanContext();

        assertEquals(span.getTraceId(), extracted.getTraceId());
        assertEquals(span.getSpanId(), extracted.getSpanId());
    }

    @Test
    void publishes_an_order_through_an_embedded_kafka_broker() {
        var producerProperties = KafkaTestUtils.producerProps(broker);
        DefaultKafkaProducerFactory<String, String> factory = new DefaultKafkaProducerFactory<>(
            producerProperties,
            new StringSerializer(),
            new StringSerializer()
        );
        KafkaTemplate<String, String> kafka = new KafkaTemplate<>(factory);
        var consumerProperties = KafkaTestUtils.consumerProps(broker, "fulfillment-test", false);
        Consumer<String, String> consumer = new org.apache.kafka.clients.consumer.KafkaConsumer<>(
            consumerProperties,
            new StringDeserializer(),
            new StringDeserializer()
        );
        try {
            broker.consumeFromAnEmbeddedTopic(consumer, "orders");
            assertEquals("published order-embedded", new OrderProducer(kafka).publish("order-embedded"));
            var record = KafkaTestUtils.getSingleRecord(consumer, "orders");
            assertEquals("order-embedded", record.value());
            consume_record_through_payment_and_notification(record);
        } finally {
            consumer.close();
            factory.destroy();
        }
    }

    private static void consume_record_through_payment_and_notification(
        org.apache.kafka.clients.consumer.ConsumerRecord<String, String> record
    ) {
        String name = InProcessServerBuilder.generateName();
        AtomicReference<QuoteRequest> request = new AtomicReference<>();
        Server server;
        try {
            server = InProcessServerBuilder.forName(name)
                .directExecutor()
                .addService(new PricingGrpc.PricingImplBase() {
                    @Override
                    public void quote(QuoteRequest value, StreamObserver<QuoteResponse> response) {
                        request.set(value);
                        response.onNext(QuoteResponse.getDefaultInstance());
                        response.onCompleted();
                    }
                })
                .build()
                .start();
        } catch (java.io.IOException error) {
            throw new AssertionError("start payment test server", error);
        }
        ManagedChannel channel = InProcessChannelBuilder.forName(name).directExecutor().build();
        NotificationClient notifications = mock(NotificationClient.class);
        try {
            new OrderConsumer(PricingGrpc.newBlockingStub(channel), notifications).onOrder(record);
            assertEquals("order-embedded", request.get().getSku());
            verify(notifications).notifyOrder();
        } finally {
            channel.shutdownNow();
            server.shutdownNow();
            try {
                channel.awaitTermination(5, TimeUnit.SECONDS);
                server.awaitTermination(5, TimeUnit.SECONDS);
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new AssertionError("stop payment test server", error);
            }
        }
    }
}
