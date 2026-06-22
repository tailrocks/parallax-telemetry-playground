package dev.tailrocks.payment;

import dev.tailrocks.pricing.v1.PricingGrpc;
import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.stub.StreamObserver;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.grpc.server.service.GrpcService;

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
    @Override
    public void quote(QuoteRequest req, StreamObserver<QuoteResponse> obs) {
        long total = 1999L * Math.max(1, req.getQuantity());
        obs.onNext(QuoteResponse.newBuilder()
            .setSku(req.getSku())
            .setQuantity(req.getQuantity())
            .setTotalMinor(total)
            .setCurrency("USD")
            .build());
        obs.onCompleted();
    }
}
