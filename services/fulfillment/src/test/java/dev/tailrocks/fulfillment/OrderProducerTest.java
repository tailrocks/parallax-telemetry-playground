package dev.tailrocks.fulfillment;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;

import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import org.springframework.kafka.core.KafkaTemplate;

@ExtendWith(OpenTelemetryTestExtension.class)
class OrderProducerTest {
    @Test
    void publishes_the_requested_order_to_the_orders_topic() {
        @SuppressWarnings("unchecked")
        KafkaTemplate<String, String> kafka = mock(KafkaTemplate.class);
        OrderProducer producer = new OrderProducer(kafka);

        assertEquals("published order-42", producer.publish("order-42"));
        verify(kafka).send("orders", "order-42");
    }
}
