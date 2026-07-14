package dev.tailrocks.payment;

import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.stub.StreamObserver;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

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

    private static final class RecordingObserver implements StreamObserver<QuoteResponse> {
        private final List<QuoteResponse> responses = new ArrayList<>();
        private Throwable error;
        private boolean completed;

        @Override public void onNext(QuoteResponse value) { responses.add(value); }
        @Override public void onError(Throwable throwable) { error = throwable; }
        @Override public void onCompleted() { completed = true; }
    }
}
