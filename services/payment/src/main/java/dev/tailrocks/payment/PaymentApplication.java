package dev.tailrocks.payment;

import dev.tailrocks.pricing.v1.PricingGrpc;
import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.stub.StreamObserver;
import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.logs.Severity;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.grpc.server.service.GrpcService;
import org.slf4j.LoggerFactory;
import org.slf4j.MDC;

@SpringBootApplication
public class PaymentApplication {
    public static void main(String[] args) {
        SpringApplication.run(PaymentApplication.class, args);
    }
}

// The Java gRPC counterpart of the Rust pricing service (same proto contract) —
// a Rust tonic client can call this Java server: cross-language gRPC. The OTel
// agent instruments it as a SERVER span on the shared trace.
@GrpcService
class PaymentPricingService extends PricingGrpc.PricingImplBase {
    private static final org.slf4j.Logger LOG = LoggerFactory.getLogger(PaymentPricingService.class);
    private static final io.opentelemetry.api.logs.Logger EVENT_LOGGER =
        GlobalOpenTelemetry.get().getLogsBridge().get("payment.events");

    @Override
    public void quote(QuoteRequest req, StreamObserver<QuoteResponse> obs) {
        String paymentMethod = "card";
        long total = 1999L * Math.max(1, req.getQuantity());
        try (
            var ignoredEvent = MDC.putCloseable(Semconv.EVENT_NAME, Semconv.PAYMENT_AUTHORIZED);
            var ignoredMethod = MDC.putCloseable("payment.method", paymentMethod)
        ) {
            LOG.atInfo()
                .addKeyValue(Semconv.EVENT_NAME, Semconv.PAYMENT_AUTHORIZED)
                .addKeyValue("payment.method", paymentMethod)
                .log("payment authorized");
        }
        emitPaymentAuthorized(paymentMethod);
        obs.onNext(QuoteResponse.newBuilder()
            .setSku(req.getSku())
            .setQuantity(req.getQuantity())
            .setTotalMinor(total)
            .setCurrency("USD")
            .build());
        obs.onCompleted();
    }

    private static void emitPaymentAuthorized(String paymentMethod) {
        EVENT_LOGGER.logRecordBuilder()
            .setEventName(Semconv.PAYMENT_AUTHORIZED)
            .setSeverity(Severity.INFO)
            .setBody(Semconv.PAYMENT_AUTHORIZED)
            .setAllAttributes(Attributes.builder()
                .put("payment.method", paymentMethod)
                .build())
            .emit();
    }
}
