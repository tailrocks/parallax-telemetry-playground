package dev.tailrocks.fulfillment;

import dev.tailrocks.pricing.v1.PricingGrpc;
import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.ManagedChannel;
import io.grpc.Server;
import io.grpc.inprocess.InProcessChannelBuilder;
import io.grpc.inprocess.InProcessServerBuilder;
import io.grpc.stub.StreamObserver;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;

@ExtendWith(OpenTelemetryTestExtension.class)
class OrderConsumerTest {
    @Test
    void consumes_an_order_through_payment_then_notifies_rust() throws Exception {
        String name = InProcessServerBuilder.generateName();
        AtomicReference<QuoteRequest> request = new AtomicReference<>();
        Server server = InProcessServerBuilder.forName(name)
            .directExecutor()
            .addService(new PricingGrpc.PricingImplBase() {
                @Override
                public void quote(QuoteRequest value, StreamObserver<QuoteResponse> response) {
                    request.set(value);
                    response.onNext(QuoteResponse.getDefaultInstance());
                    response.onCompleted();
                }
            })
            .build()
            .start();
        ManagedChannel channel = InProcessChannelBuilder.forName(name).directExecutor().build();
        NotificationClient notifications = mock(NotificationClient.class);

        try {
            new OrderConsumer(PricingGrpc.newBlockingStub(channel), notifications).onOrder("WIDGET-1");

            assertEquals("WIDGET-1", request.get().getSku());
            assertEquals(1, request.get().getQuantity());
            verify(notifications).notifyOrder();
        } finally {
            channel.shutdownNow();
            server.shutdownNow();
            channel.awaitTermination(5, TimeUnit.SECONDS);
            server.awaitTermination(5, TimeUnit.SECONDS);
        }
    }
}
