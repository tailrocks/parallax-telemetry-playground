package dev.tailrocks.fulfillment;

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.stereotype.Component;
import org.springframework.web.client.RestClient;

@SpringBootApplication
public class FulfillmentApplication {
    public static void main(String[] args) {
        SpringApplication.run(FulfillmentApplication.class, args);
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
