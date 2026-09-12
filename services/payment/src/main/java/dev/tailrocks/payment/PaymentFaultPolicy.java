package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.PaymentFailureReason;
import dev.tailrocks.payment.v1.PaymentMethod;
import dev.tailrocks.payment.v1.PaymentOperationStatus;
import io.grpc.Status;
import java.util.Locale;

/** Deterministic, token-only provider behavior for reproducible playground runs. */
final class PaymentFaultPolicy {
    private PaymentFaultPolicy() {}

    enum Kind {
        NONE,
        DECLINED,
        INSUFFICIENT_FUNDS,
        INVALID_PAYMENT_METHOD,
        PENDING,
        UNAVAILABLE,
        DEADLINE_EXCEEDED,
        INTERNAL
    }

    enum PendingResolution {
        AUTHORIZED("authorized"),
        FAILED("failed"),
        MANUAL_RECONCILIATION("manual_reconciliation");

        private final String databaseValue;

        PendingResolution(String databaseValue) {
            this.databaseValue = databaseValue;
        }

        String databaseValue() {
            return databaseValue;
        }

        boolean hasProviderDecision() {
            return this == AUTHORIZED || this == FAILED;
        }
    }

    record Decision(
        Kind kind,
        PaymentOperationStatus operationStatus,
        PaymentFailureReason failureReason,
        PendingResolution pendingResolution
    ) {
        boolean isTransportFailure() {
            return switch (kind) {
                case UNAVAILABLE, DEADLINE_EXCEEDED, INTERNAL -> true;
                default -> false;
            };
        }

        PaymentRpcException transportException() {
            return switch (kind) {
                case UNAVAILABLE -> new PaymentRpcException(
                    Status.Code.UNAVAILABLE,
                    failureReason,
                    "payment provider unavailable"
                );
                case DEADLINE_EXCEEDED -> new PaymentRpcException(
                    Status.Code.DEADLINE_EXCEEDED,
                    failureReason,
                    "payment provider timed out"
                );
                case INTERNAL -> new PaymentRpcException(
                    Status.Code.INTERNAL,
                    failureReason,
                    "payment provider returned an internal error"
                );
                default -> throw new IllegalStateException("not a transport fault: " + kind);
            };
        }
    }

    static Decision forMethod(PaymentMethod method) {
        String token = method.getToken().strip().toLowerCase(Locale.ROOT);
        return switch (token) {
            case "tok_decline", "tok_declined", "card_declined", "decline" ->
                new Decision(
                    Kind.DECLINED,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_DECLINED,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_DECLINED,
                    null
                );
            case "tok_insufficient_funds", "insufficient_funds" ->
                new Decision(
                    Kind.INSUFFICIENT_FUNDS,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_DECLINED,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_INSUFFICIENT_FUNDS,
                    null
                );
            case "tok_invalid", "tok_invalid_payment_method", "invalid_payment_method" ->
                new Decision(
                    Kind.INVALID_PAYMENT_METHOD,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_DECLINED,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_INVALID_PAYMENT_METHOD,
                    null
                );
            case "tok_pending", "provider_pending" ->
                new Decision(
                    Kind.PENDING,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_PENDING,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                    PendingResolution.AUTHORIZED
                );
            case "tok_pending_fail", "provider_pending_fail" ->
                new Decision(
                    Kind.PENDING,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_PENDING,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                    PendingResolution.FAILED
                );
            case "tok_pending_unknown", "provider_pending_unknown" ->
                new Decision(
                    Kind.PENDING,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_PENDING,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE,
                    PendingResolution.MANUAL_RECONCILIATION
                );
            case "tok_unavailable", "tok_provider_unavailable", "fault_unavailable" ->
                new Decision(
                    Kind.UNAVAILABLE,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_FAILED,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE,
                    null
                );
            case "tok_timeout", "fault_timeout" ->
                new Decision(
                    Kind.DEADLINE_EXCEEDED,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_FAILED,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE,
                    null
                );
            case "tok_internal", "fault_internal" ->
                new Decision(
                    Kind.INTERNAL,
                    PaymentOperationStatus.PAYMENT_OPERATION_STATUS_FAILED,
                    PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                    null
                );
            default -> new Decision(
                Kind.NONE,
                PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED,
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                null
            );
        };
    }
}
