package dev.tailrocks.payment;

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.util.Map;

@SpringBootApplication
public class PaymentApplication {
    public static void main(String[] args) {
        SpringApplication.run(PaymentApplication.class, args);
    }
}

// HTTP charge endpoint today (a SERVER span); the gRPC Charge service that
// checkout calls is the next step (proto codegen + spring-grpc). Deliberate
// failure chaos (paymentFailure flag) hangs off here.
@RestController
class PaymentController {
    @GetMapping("/charge")
    Map<String, Object> charge(@RequestParam String sku, @RequestParam long amountMinor) {
        return Map.of("sku", sku, "amountMinor", amountMinor, "status", "captured");
    }
}
