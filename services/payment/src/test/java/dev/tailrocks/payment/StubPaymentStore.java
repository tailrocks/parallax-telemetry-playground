package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.AuthorizeResponse;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.CaptureResponse;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.GetPaymentResponse;
import dev.tailrocks.payment.v1.Money;
import dev.tailrocks.payment.v1.PaymentFailureReason;
import dev.tailrocks.payment.v1.PaymentOperationStatus;
import dev.tailrocks.payment.v1.PaymentRecord;
import dev.tailrocks.payment.v1.PaymentStatus;
import dev.tailrocks.payment.v1.RefundRequest;
import dev.tailrocks.payment.v1.RefundResponse;
import dev.tailrocks.payment.v1.VoidRequest;
import dev.tailrocks.payment.v1.VoidResponse;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Test-only transport double; production uses JdbcPaymentStore and PostgreSQL. */
final class StubPaymentStore implements PaymentStore {
    private final Map<String, PaymentRecord> payments = new HashMap<>();
    private final Map<String, AuthorizeResponse> authorizeResponses = new HashMap<>();
    private final Map<String, CaptureResponse> captureResponses = new HashMap<>();
    private final Map<String, VoidResponse> voidResponses = new HashMap<>();
    private final Map<String, RefundResponse> refundResponses = new HashMap<>();
    private final Map<String, String> paymentTenants = new HashMap<>();
    private final Map<String, Boolean> providerDecisionProof = new HashMap<>();
    private final Map<String, PaymentFaultPolicy.PendingResolution> pendingResolutions = new HashMap<>();
    private final Map<String, String> authorizeKeysByPaymentId = new HashMap<>();
    private int authorizeCalls;
    private int nextPayment = 1;

    @Override
    public AuthorizeResponse authorize(
        AuthorizeRequest request,
        PaymentFaultPolicy.Decision decision
    ) {
        AuthorizeResponse existing = authorizeResponses.get(operationKey(request.getTenantId(), request.getRequestId()));
        if (existing != null) {
            return existing;
        }
        authorizeCalls++;
        String id = "pay_test_" + nextPayment++;
        PaymentStatus status = decision.kind() == PaymentFaultPolicy.Kind.PENDING
            ? PaymentStatus.PAYMENT_STATUS_PENDING
            : decision.kind() == PaymentFaultPolicy.Kind.NONE
                ? PaymentStatus.PAYMENT_STATUS_AUTHORIZED
                : PaymentStatus.PAYMENT_STATUS_FAILED;
        PaymentRecord payment = PaymentRecord.newBuilder()
            .setPaymentId(id)
            .setMerchantReference(request.getMerchantReference())
            .setAuthorizedAmount(request.getAmount())
            .setStatus(status)
            .setMethodType(request.getPaymentMethod().getType())
            .setFailureReason(decision.failureReason())
            .setProviderReference("provider_test_" + nextPayment)
            .setCapturedAmount(zero(request.getAmount().getCurrencyCode()))
            .setRefundedAmount(zero(request.getAmount().getCurrencyCode()))
            .build();
        AuthorizeResponse response = AuthorizeResponse.newBuilder()
            .setOperationStatus(decision.operationStatus())
            .setPayment(payment)
            .setFailureReason(decision.failureReason())
            .build();
        payments.put(id, payment);
        paymentTenants.put(id, request.getTenantId());
        providerDecisionProof.put(id, decision.kind() == PaymentFaultPolicy.Kind.NONE);
        String operationKey = operationKey(request.getTenantId(), request.getRequestId());
        authorizeResponses.put(operationKey, response);
        if (decision.kind() == PaymentFaultPolicy.Kind.PENDING) {
            pendingResolutions.put(id, decision.pendingResolution());
            authorizeKeysByPaymentId.put(id, operationKey);
        }
        return response;
    }

    @Override
    public CaptureResponse capture(CaptureRequest request) {
        CaptureResponse existing = captureResponses.get(operationKey(request.getTenantId(), request.getRequestId()));
        if (existing != null) {
            return existing;
        }
        PaymentRecord current = requirePayment(request.getTenantId(), request.getPaymentId());
        if (!providerDecisionProof.getOrDefault(request.getPaymentId(), false)
            || current.getStatus() != PaymentStatus.PAYMENT_STATUS_AUTHORIZED) {
            throw PaymentRpcException.failedPrecondition("payment is not authorized");
        }
        long amount = request.hasAmount()
            ? request.getAmount().getAmountMinor()
            : current.getAuthorizedAmount().getAmountMinor() - current.getCapturedAmount().getAmountMinor();
        long captured = current.getCapturedAmount().getAmountMinor() + amount;
        PaymentRecord updated = current.toBuilder()
            .setStatus(captured == current.getAuthorizedAmount().getAmountMinor()
                ? PaymentStatus.PAYMENT_STATUS_CAPTURED
                : PaymentStatus.PAYMENT_STATUS_AUTHORIZED)
            .setCapturedAmount(money(current.getAuthorizedAmount().getCurrencyCode(), captured))
            .build();
        payments.put(updated.getPaymentId(), updated);
        CaptureResponse response = CaptureResponse.newBuilder()
            .setOperationStatus(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED)
            .setPayment(updated)
            .setFailureReason(PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED)
            .build();
        captureResponses.put(operationKey(request.getTenantId(), request.getRequestId()), response);
        return response;
    }

    @Override
    public VoidResponse voidPayment(VoidRequest request) {
        VoidResponse existing = voidResponses.get(operationKey(request.getTenantId(), request.getRequestId()));
        if (existing != null) {
            return existing;
        }
        PaymentRecord updated = requirePayment(request.getTenantId(), request.getPaymentId()).toBuilder()
            .setStatus(PaymentStatus.PAYMENT_STATUS_VOIDED)
            .build();
        payments.put(updated.getPaymentId(), updated);
        VoidResponse response = VoidResponse.newBuilder()
            .setOperationStatus(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED)
            .setPayment(updated)
            .setFailureReason(PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED)
            .build();
        voidResponses.put(operationKey(request.getTenantId(), request.getRequestId()), response);
        return response;
    }

    @Override
    public RefundResponse refund(RefundRequest request) {
        RefundResponse existing = refundResponses.get(operationKey(request.getTenantId(), request.getRequestId()));
        if (existing != null) {
            return existing;
        }
        PaymentRecord current = requirePayment(request.getTenantId(), request.getPaymentId());
        long refunded = current.getRefundedAmount().getAmountMinor() + request.getAmount().getAmountMinor();
        PaymentRecord updated = current.toBuilder()
            .setStatus(refunded == current.getCapturedAmount().getAmountMinor()
                ? PaymentStatus.PAYMENT_STATUS_REFUNDED
                : PaymentStatus.PAYMENT_STATUS_PARTIALLY_REFUNDED)
            .setRefundedAmount(money(request.getAmount().getCurrencyCode(), refunded))
            .build();
        payments.put(updated.getPaymentId(), updated);
        RefundResponse response = RefundResponse.newBuilder()
            .setOperationStatus(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED)
            .setPayment(updated)
            .setFailureReason(PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED)
            .build();
        refundResponses.put(operationKey(request.getTenantId(), request.getRequestId()), response);
        return response;
    }

    @Override
    public GetPaymentResponse getPayment(GetPaymentRequest request) {
        reconcilePendingPayment(request.getTenantId(), request.getPaymentId());
        PaymentRecord payment = requirePayment(request.getTenantId(), request.getPaymentId());
        return GetPaymentResponse.newBuilder()
            .setPayment(payment)
            .setStatus(payment.getStatus())
            .build();
    }

    @Override
    public synchronized int reconcilePendingPayments(int maxBatch) {
        if (maxBatch <= 0) {
            throw new IllegalArgumentException("payment reconciliation batch size must be positive");
        }
        int reconciled = 0;
        for (String paymentId : List.copyOf(pendingResolutions.keySet())) {
            if (reconciled >= Math.min(maxBatch, 100)) {
                break;
            }
            reconciled += reconcilePendingPayment(paymentTenants.get(paymentId), paymentId);
        }
        return reconciled;
    }

    @Override
    public synchronized int reconcilePendingPayment(String tenantId, String paymentId) {
        PaymentRecord current = payments.get(paymentId);
        PaymentFaultPolicy.PendingResolution resolution = pendingResolutions.get(paymentId);
        if (current == null
            || resolution == null
            || !tenantId.equals(paymentTenants.get(paymentId))
            || current.getStatus() != PaymentStatus.PAYMENT_STATUS_PENDING) {
            return 0;
        }
        if (resolution == PaymentFaultPolicy.PendingResolution.MANUAL_RECONCILIATION) {
            return 0;
        }
        boolean authorized = resolution == PaymentFaultPolicy.PendingResolution.AUTHORIZED;
        PaymentFailureReason failureReason = authorized
            ? PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED
            : PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE;
        PaymentRecord updated = current.toBuilder()
            .setStatus(authorized
                ? PaymentStatus.PAYMENT_STATUS_AUTHORIZED
                : PaymentStatus.PAYMENT_STATUS_FAILED)
            .setFailureReason(failureReason)
            .build();
        payments.put(paymentId, updated);
        providerDecisionProof.put(paymentId, true);
        String operationKey = authorizeKeysByPaymentId.get(paymentId);
        AuthorizeResponse response = authorizeResponses.get(operationKey).toBuilder()
            .setOperationStatus(authorized
                ? PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED
                : PaymentOperationStatus.PAYMENT_OPERATION_STATUS_FAILED)
            .setPayment(updated)
            .setFailureReason(failureReason)
            .build();
        authorizeResponses.put(operationKey, response);
        pendingResolutions.remove(paymentId);
        authorizeKeysByPaymentId.remove(paymentId);
        return 1;
    }

    int authorizeCalls() {
        return authorizeCalls;
    }

    private PaymentRecord requirePayment(String tenantId, String id) {
        PaymentRecord payment = payments.get(id);
        if (payment == null || !tenantId.equals(paymentTenants.get(id))) {
            throw PaymentRpcException.notFound("payment was not found");
        }
        return payment;
    }

    private static String operationKey(String tenantId, String requestId) {
        return tenantId + ":" + requestId;
    }

    private static Money zero(String currency) {
        return money(currency, 0);
    }

    private static Money money(String currency, long amount) {
        return Money.newBuilder().setCurrencyCode(currency).setAmountMinor(amount).build();
    }
}
