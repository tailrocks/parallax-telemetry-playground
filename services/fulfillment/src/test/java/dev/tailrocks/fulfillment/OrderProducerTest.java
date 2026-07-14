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
import org.apache.kafka.clients.producer.ProducerRecord;
import org.apache.kafka.common.header.internals.RecordHeaders;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import org.springframework.kafka.core.KafkaTemplate;
import org.mockito.ArgumentCaptor;

@ExtendWith(OpenTelemetryTestExtension.class)
class OrderProducerTest {
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
}
