package dev.tailrocks.payment;

import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.stub.StreamObserver;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@ExtendWith(OpenTelemetryTestExtension.class)
class PaymentPricingServiceTest {
    @Test
    void streamsOrderedQuotesAndReportsRequestedFailure() {
        PaymentPricingService service = new PaymentPricingService();
        RecordingObserver observer = new RecordingObserver();
        service.quoteStream(QuoteRequest.newBuilder().setSku("WIDGET-1").setQuantity(3).build(), observer);
        assertEquals(List.of(1, 2, 3), observer.responses.stream().map(QuoteResponse::getQuantity).toList());
        assertTrue(observer.completed);

        RecordingObserver failing = new RecordingObserver();
        service.quoteStream(QuoteRequest.newBuilder().setQuantity(3).setFailAt(2).build(), failing);
        assertEquals(List.of(1), failing.responses.stream().map(QuoteResponse::getQuantity).toList());
        assertNotNull(failing.error);
    }

    @Test
    void exposes_the_shared_payment_error_from_the_unary_endpoint() {
        RecordingObserver observer = new RecordingObserver();
        new PaymentPricingService().quote(
            QuoteRequest.newBuilder().setSku("PAYMENT-ERROR").setQuantity(1).build(),
            observer
        );
        assertTrue(observer.responses.isEmpty());
        assertNotNull(observer.error);
        assertTrue(observer.error.getMessage().contains("PaymentError: payment failed"));
    }

    @ParameterizedTest(name = "quantity={0}")
    @ValueSource(ints = {1, 3})
    void preserves_parameterized_quote_variants(int quantity) {
        RecordingObserver observer = new RecordingObserver();
        new PaymentPricingService().quote(
            QuoteRequest.newBuilder().setSku("WIDGET-1").setQuantity(quantity).build(),
            observer
        );
        assertEquals(quantity, observer.responses.getFirst().getQuantity());
    }

    private static final class RecordingObserver implements StreamObserver<QuoteResponse> {
        private final List<QuoteResponse> responses = new ArrayList<>();
        private Throwable error;
        private boolean completed;

        @Override public void onNext(QuoteResponse value) { responses.add(value); }
        @Override public void onError(Throwable throwable) { error = throwable; }
        @Override public void onCompleted() { completed = true; }
    }
}
