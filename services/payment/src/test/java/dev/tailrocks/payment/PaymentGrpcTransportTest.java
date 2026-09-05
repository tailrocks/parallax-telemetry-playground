package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.Money;
import dev.tailrocks.payment.v1.PaymentGrpc;
import dev.tailrocks.payment.v1.PaymentMethod;
import dev.tailrocks.payment.v1.PaymentMethodType;
import dev.tailrocks.payment.v1.PaymentStatus;
import dev.tailrocks.payment.v1.RefundRequest;
import dev.tailrocks.payment.v1.RefundReason;
import dev.tailrocks.payment.v1.VoidRequest;
import dev.tailrocks.payment.v1.VoidReason;
import io.grpc.ManagedChannel;
import io.grpc.Server;
import io.grpc.StatusRuntimeException;
import io.grpc.inprocess.InProcessChannelBuilder;
import io.grpc.inprocess.InProcessServerBuilder;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

@ExtendWith(OpenTelemetryTestExtension.class)
class PaymentGrpcTransportTest {
    private Server server;
    private ManagedChannel channel;

    @BeforeEach
    void startServer() throws Exception {
        String name = InProcessServerBuilder.generateName();
        server = InProcessServerBuilder.forName(name)
            .directExecutor()
            .addService(new PaymentService(new StubPaymentStore()))
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
    void serves_authorize_capture_void_refund_and_get_over_payment_grpc() {
        PaymentGrpc.PaymentBlockingStub client = PaymentGrpc.newBlockingStub(channel);
        var authorized = client.authorize(AuthorizeRequest.newBuilder()
            .setRequestId("auth-transport")
            .setTenantId("tenant-acme")
            .setMerchantReference("order-transport")
            .setAmount(money(4000))
            .setPaymentMethod(card("tok_transport"))
            .build());
        assertEquals(PaymentStatus.PAYMENT_STATUS_AUTHORIZED, authorized.getPayment().getStatus());

        var captured = client.capture(CaptureRequest.newBuilder()
            .setRequestId("capture-transport")
            .setTenantId("tenant-acme")
            .setPaymentId(authorized.getPayment().getPaymentId())
            .setAmount(money(4000))
            .build());
        assertEquals(PaymentStatus.PAYMENT_STATUS_CAPTURED, captured.getPayment().getStatus());

        var partialRefund = client.refund(RefundRequest.newBuilder()
            .setRequestId("refund-transport")
            .setTenantId("tenant-acme")
            .setPaymentId(authorized.getPayment().getPaymentId())
            .setAmount(money(1000))
            .setReason(RefundReason.REFUND_REASON_ORDER_CANCELLED)
            .build());
        assertEquals(
            PaymentStatus.PAYMENT_STATUS_PARTIALLY_REFUNDED,
            partialRefund.getPayment().getStatus()
        );

        var refunded = client.refund(RefundRequest.newBuilder()
            .setRequestId("refund-transport-final")
            .setTenantId("tenant-acme")
            .setPaymentId(authorized.getPayment().getPaymentId())
            .setAmount(money(3000))
            .setReason(RefundReason.REFUND_REASON_ORDER_CANCELLED)
            .build());
        assertEquals(PaymentStatus.PAYMENT_STATUS_REFUNDED, refunded.getPayment().getStatus());
        assertEquals(
            PaymentStatus.PAYMENT_STATUS_REFUNDED,
            client.getPayment(GetPaymentRequest.newBuilder()
                .setPaymentId(authorized.getPayment().getPaymentId())
                .setTenantId("tenant-acme")
                .build())
                .getStatus()
        );
        StatusRuntimeException wrongTenant = assertThrows(
            StatusRuntimeException.class,
            () -> client.getPayment(GetPaymentRequest.newBuilder()
                .setPaymentId(authorized.getPayment().getPaymentId())
                .setTenantId("tenant-nova")
                .build())
        );
        assertEquals(io.grpc.Status.Code.NOT_FOUND, wrongTenant.getStatus().getCode());

        var voided = client.authorize(AuthorizeRequest.newBuilder()
            .setRequestId("auth-void-transport")
            .setTenantId("tenant-acme")
            .setMerchantReference("order-void-transport")
            .setAmount(money(1000))
            .setPaymentMethod(card("tok_void_transport"))
            .build());
        assertEquals(
            PaymentStatus.PAYMENT_STATUS_VOIDED,
            client.void_(VoidRequest.newBuilder()
                .setRequestId("void-transport")
                .setTenantId("tenant-acme")
                .setPaymentId(voided.getPayment().getPaymentId())
                .setReason(VoidReason.VOID_REASON_CUSTOMER_CANCELLED)
                .build())
                .getPayment()
                .getStatus()
        );
    }

    private static Money money(long amountMinor) {
        return Money.newBuilder().setCurrencyCode("USD").setAmountMinor(amountMinor).build();
    }

    private static PaymentMethod card(String token) {
        return PaymentMethod.newBuilder()
            .setType(PaymentMethodType.PAYMENT_METHOD_TYPE_CARD)
            .setToken(token)
            .build();
    }
}
