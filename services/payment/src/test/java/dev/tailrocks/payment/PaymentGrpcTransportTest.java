package dev.tailrocks.payment;

import dev.tailrocks.pricing.v1.PricingGrpc;
import dev.tailrocks.pricing.v1.QuoteRequest;
import dev.tailrocks.pricing.v1.QuoteResponse;
import io.grpc.ManagedChannel;
import io.grpc.Server;
import io.grpc.inprocess.InProcessChannelBuilder;
import io.grpc.inprocess.InProcessServerBuilder;
import java.util.Iterator;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;

class PaymentGrpcTransportTest {
    private Server server;
    private ManagedChannel channel;

    @BeforeEach
    void startServer() throws Exception {
        String name = InProcessServerBuilder.generateName();
        server = InProcessServerBuilder.forName(name)
            .directExecutor()
            .addService(new PaymentPricingService())
            .build()
            .start();
        channel = InProcessChannelBuilder.forName(name).directExecutor().build();
    }

    @AfterEach
    void stopServer() throws Exception {
        channel.shutdownNow();
        server.shutdownNow();
        channel.awaitTermination(5, TimeUnit.SECONDS);
        server.awaitTermination(5, TimeUnit.SECONDS);
    }

    @Test
    void serves_unary_and_streaming_pricing_over_grpc_transport() {
        PricingGrpc.PricingBlockingStub client = PricingGrpc.newBlockingStub(channel);
        QuoteResponse unary = client.quote(
            QuoteRequest.newBuilder().setSku("WIDGET-1").setQuantity(2).build()
        );
        assertEquals(3998, unary.getTotalMinor());

        Iterator<QuoteResponse> stream = client.quoteStream(
            QuoteRequest.newBuilder().setSku("WIDGET-1").setQuantity(3).build()
        );
        assertEquals(1, stream.next().getQuantity());
        assertEquals(2, stream.next().getQuantity());
        assertEquals(3, stream.next().getQuantity());
        assertFalse(stream.hasNext());
    }
}
