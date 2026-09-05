package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.AuthorizeResponse;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.CaptureResponse;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.Money;
import dev.tailrocks.payment.v1.PaymentFailureReason;
import dev.tailrocks.payment.v1.PaymentMethod;
import dev.tailrocks.payment.v1.PaymentMethodType;
import dev.tailrocks.payment.v1.PaymentOperationStatus;
import dev.tailrocks.payment.v1.PaymentStatus;
import io.grpc.Status;
import io.grpc.StatusRuntimeException;
import io.grpc.stub.StreamObserver;
import io.tailrocks.testsupport.OpenTelemetryTestExtension;
import java.util.ArrayList;
import java.util.List;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

@ExtendWith(OpenTelemetryTestExtension.class)
class PaymentServiceTest {
    @Test
    void authorize_uses_the_payment_contract_and_replays_the_same_result() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        AuthorizeRequest request = authorize("auth-1", "tenant-acme", "order-1", "tok_live");

        RecordingObserver<AuthorizeResponse> first = new RecordingObserver<>();
        service.authorize(request, first);
        RecordingObserver<AuthorizeResponse> retry = new RecordingObserver<>();
        service.authorize(request, retry);

        assertTrue(first.completed);
        assertTrue(retry.completed);
        assertEquals(first.responses, retry.responses);
        assertEquals(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED,
            first.responses.getFirst().getOperationStatus());
        assertEquals("pay_test_1", first.responses.getFirst().getPayment().getPaymentId());
        assertEquals(1, store.authorizeCalls());
    }

    @Test
    void decline_is_a_typed_operation_outcome() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        RecordingObserver<AuthorizeResponse> observer = new RecordingObserver<>();

        service.authorize(
            authorize("auth-decline", "tenant-acme", "order-decline", "tok_decline"),
            observer
        );

        assertTrue(observer.completed);
        assertEquals(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_DECLINED,
            observer.responses.getFirst().getOperationStatus());
        assertEquals(PaymentFailureReason.PAYMENT_FAILURE_REASON_DECLINED,
            observer.responses.getFirst().getFailureReason());
        assertEquals("pay_test_1", observer.responses.getFirst().getPayment().getPaymentId());
    }

    @Test
    void provider_fault_token_returns_retryable_typed_grpc_error() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        RecordingObserver<AuthorizeResponse> observer = new RecordingObserver<>();

        service.authorize(
            authorize("auth-unavailable", "tenant-acme", "order-unavailable", "tok_unavailable"),
            observer
        );

        StatusRuntimeException error = assertInstanceOf(StatusRuntimeException.class, observer.error);
        assertEquals(Status.Code.UNAVAILABLE, error.getStatus().getCode());
        assertNotNull(error.getTrailers());
        assertEquals(
            PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE.name(),
            error.getTrailers().get(PaymentService.FAILURE_REASON)
        );
        assertTrue(observer.responses.isEmpty());
        assertTrue(!observer.completed);
        assertEquals(0, store.authorizeCalls());
    }

    @Test
    void pending_authorization_reconciliation_updates_get_and_authorize_replay() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        AuthorizeRequest request = authorize(
            "auth-pending",
            "tenant-acme",
            "order-pending",
            "tok_pending"
        );

        RecordingObserver<AuthorizeResponse> first = new RecordingObserver<>();
        service.authorize(request, first);
        assertEquals(
            PaymentOperationStatus.PAYMENT_OPERATION_STATUS_PENDING,
            first.responses.getFirst().getOperationStatus()
        );

        assertEquals(1, store.reconcilePendingPayments(1));

        RecordingObserver<AuthorizeResponse> retry = new RecordingObserver<>();
        service.authorize(request, retry);
        assertEquals(
            PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED,
            retry.responses.getFirst().getOperationStatus()
        );
        assertEquals(
            PaymentStatus.PAYMENT_STATUS_AUTHORIZED,
            retry.responses.getFirst().getPayment().getStatus()
        );

        RecordingObserver<dev.tailrocks.payment.v1.GetPaymentResponse> read = new RecordingObserver<>();
        service.getPayment(
            GetPaymentRequest.newBuilder()
                .setTenantId("tenant-acme")
                .setPaymentId(retry.responses.getFirst().getPayment().getPaymentId())
                .build(),
            read
        );
        assertEquals(PaymentStatus.PAYMENT_STATUS_AUTHORIZED, read.responses.getFirst().getStatus());
        assertEquals(0, store.reconcilePendingPayments(1));
    }

    @Test
    void pending_provider_failure_converges_to_failed_without_recreating_a_payment() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        AuthorizeRequest request = authorize(
            "auth-pending-fail",
            "tenant-acme",
            "order-pending-fail",
            "tok_pending_fail"
        );

        RecordingObserver<AuthorizeResponse> first = new RecordingObserver<>();
        service.authorize(request, first);
        String paymentId = first.responses.getFirst().getPayment().getPaymentId();
        assertEquals(1, store.reconcilePendingPayment("tenant-acme", paymentId));

        RecordingObserver<AuthorizeResponse> retry = new RecordingObserver<>();
        service.authorize(request, retry);
        assertEquals(
            PaymentOperationStatus.PAYMENT_OPERATION_STATUS_FAILED,
            retry.responses.getFirst().getOperationStatus()
        );
        assertEquals(PaymentStatus.PAYMENT_STATUS_FAILED, retry.responses.getFirst().getPayment().getStatus());
        assertEquals(
            PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE,
            retry.responses.getFirst().getFailureReason()
        );
        assertEquals(1, store.authorizeCalls());
    }

    @Test
    void unknown_provider_outcome_stays_pending_and_cannot_be_captured() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        AuthorizeRequest request = authorize(
            "auth-pending-unknown",
            "tenant-acme",
            "order-pending-unknown",
            "tok_pending_unknown"
        );

        RecordingObserver<AuthorizeResponse> authorization = new RecordingObserver<>();
        service.authorize(request, authorization);
        String paymentId = authorization.responses.getFirst().getPayment().getPaymentId();
        assertEquals(PaymentStatus.PAYMENT_STATUS_PENDING,
            authorization.responses.getFirst().getPayment().getStatus());
        assertEquals(0, store.reconcilePendingPayment("tenant-acme", paymentId));

        RecordingObserver<CaptureResponse> capture = new RecordingObserver<>();
        service.capture(CaptureRequest.newBuilder()
            .setRequestId("capture-pending-unknown")
            .setTenantId("tenant-acme")
            .setPaymentId(paymentId)
            .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2500))
            .build(), capture);

        StatusRuntimeException error = assertInstanceOf(StatusRuntimeException.class, capture.error);
        assertEquals(Status.Code.FAILED_PRECONDITION, error.getStatus().getCode());
        assertTrue(capture.responses.isEmpty());
    }

    @Test
    void malformed_requests_fail_before_persistence() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        RecordingObserver<AuthorizeResponse> observer = new RecordingObserver<>();

        service.authorize(AuthorizeRequest.getDefaultInstance(), observer);

        StatusRuntimeException error = assertInstanceOf(StatusRuntimeException.class, observer.error);
        assertEquals(Status.Code.INVALID_ARGUMENT, error.getStatus().getCode());
        assertEquals(0, store.authorizeCalls());
    }

    @Test
    void authorize_requires_tenant_context() {
        StubPaymentStore store = new StubPaymentStore();
        PaymentService service = new PaymentService(store);
        RecordingObserver<AuthorizeResponse> observer = new RecordingObserver<>();

        service.authorize(
            authorize("auth-no-tenant", "tenant-acme", "order-no-tenant", "tok_live")
                .toBuilder()
                .clearTenantId()
                .build(),
            observer
        );

        StatusRuntimeException error = assertInstanceOf(StatusRuntimeException.class, observer.error);
        assertEquals(Status.Code.INVALID_ARGUMENT, error.getStatus().getCode());
        assertEquals(0, store.authorizeCalls());
    }

    private static AuthorizeRequest authorize(
        String requestId,
        String tenantId,
        String orderId,
        String token
    ) {
        return AuthorizeRequest.newBuilder()
            .setRequestId(requestId)
            .setTenantId(tenantId)
            .setMerchantReference(orderId)
            .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2500))
            .setPaymentMethod(PaymentMethod.newBuilder()
                .setType(PaymentMethodType.PAYMENT_METHOD_TYPE_CARD)
                .setToken(token))
            .build();
    }

    static final class RecordingObserver<T> implements StreamObserver<T> {
        private final List<T> responses = new ArrayList<>();
        private Throwable error;
        private boolean completed;

        @Override public void onNext(T value) { responses.add(value); }
        @Override public void onError(Throwable throwable) { error = throwable; }
        @Override public void onCompleted() { completed = true; }
    }
}
