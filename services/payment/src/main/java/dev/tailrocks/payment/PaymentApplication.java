package dev.tailrocks.payment;

import io.tailrocks.semconv.Semconv;

import dev.tailrocks.pricing.v1.PricingGrpc;
import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.stub.StreamObserver;
import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.common.Attributes;
import io.opentelemetry.api.logs.Severity;
import io.opentelemetry.api.trace.Span;
import io.opentelemetry.api.trace.StatusCode;
import io.sentry.Sentry;
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
        if ("PAYMENT-ERROR".equals(req.getSku())) {
            PaymentError error = new PaymentError();
            Span.current().recordException(error);
            Span.current().setStatus(StatusCode.ERROR, error.getMessage());
            Sentry.captureException(error);
            LOG.atError()
                .addKeyValue(Semconv.ERROR_TYPE, "PaymentError")
                .addKeyValue("error.message", error.getMessage())
                .setCause(error)
                .log("payment failure (chaos)");
            obs.onError(io.grpc.Status.INTERNAL
                .withDescription(error.getMessage())
                .withCause(error)
                .asRuntimeException());
            return;
        }
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

    @Override
    public void quoteStream(QuoteRequest req, StreamObserver<QuoteResponse> obs) {
        int count = Math.max(1, req.getQuantity());
        for (int item = 1; item <= count; item++) {
            if (req.getFailAt() > 0 && item == req.getFailAt()) {
                obs.onError(io.grpc.Status.INTERNAL
                    .withDescription("pricing stream failed at requested item")
                    .asRuntimeException());
                return;
            }
            if (req.getDelayMs() > 0) {
                try {
                    Thread.sleep(req.getDelayMs());
                } catch (InterruptedException error) {
                    Thread.currentThread().interrupt();
                    obs.onError(error);
                    return;
                }
            }
            obs.onNext(QuoteResponse.newBuilder()
                .setSku(req.getSku())
                .setQuantity(item)
                .setTotalMinor(1999L * item)
                .setCurrency("USD")
                .build());
        }
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

final class PaymentError extends RuntimeException {
    PaymentError() {
        super("PaymentError: payment failed");
    }
}
