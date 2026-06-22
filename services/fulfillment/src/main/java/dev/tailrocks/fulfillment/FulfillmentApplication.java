package dev.tailrocks.fulfillment;

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
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

    @PostMapping("/publish")
    String publish(@RequestParam(defaultValue = "order-1") String order) {
        kafka.send("orders", order);
        return "published " + order;
    }
}

@Component
class OrderConsumer {
    private final RestClient http = RestClient.create();
    private final String notificationsUrl =
        System.getenv().getOrDefault("NOTIFICATIONS_URL", "http://notifications:8091");

    // CONSUMER span (auto-instrumented); the reverse Java→Rust hop follows.
    @KafkaListener(topics = "orders", groupId = "fulfillment")
    void onOrder(String order) {
        http.get().uri(notificationsUrl + "/").retrieve().toBodilessEntity();
    }
}
