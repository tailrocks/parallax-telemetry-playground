package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.PaymentFailureReason;
import io.grpc.Status;

/** A typed business or transport failure returned by the payment boundary. */
final class PaymentRpcException extends RuntimeException {
    private final Status.Code statusCode;
    private final PaymentFailureReason failureReason;

    PaymentRpcException(
        Status.Code statusCode,
        PaymentFailureReason failureReason,
        String message
    ) {
        super(message);
        this.statusCode = statusCode;
        this.failureReason = failureReason;
    }

    PaymentRpcException(
        Status.Code statusCode,
        PaymentFailureReason failureReason,
        String message,
        Throwable cause
    ) {
        super(message, cause);
        this.statusCode = statusCode;
        this.failureReason = failureReason;
    }

    Status.Code statusCode() {
        return statusCode;
    }

    PaymentFailureReason failureReason() {
        return failureReason;
    }

    static PaymentRpcException invalidArgument(
        PaymentFailureReason reason,
        String message
    ) {
        return new PaymentRpcException(Status.Code.INVALID_ARGUMENT, reason, message);
    }

    static PaymentRpcException alreadyExists(String message) {
        return new PaymentRpcException(
            Status.Code.ALREADY_EXISTS,
            PaymentFailureReason.PAYMENT_FAILURE_REASON_ALREADY_PROCESSED,
            message
        );
    }

    static PaymentRpcException notFound(String message) {
        return new PaymentRpcException(
            Status.Code.NOT_FOUND,
            PaymentFailureReason.PAYMENT_FAILURE_REASON_INVALID_STATE,
            message
        );
    }

    static PaymentRpcException failedPrecondition(String message) {
        return new PaymentRpcException(
            Status.Code.FAILED_PRECONDITION,
            PaymentFailureReason.PAYMENT_FAILURE_REASON_INVALID_STATE,
            message
        );
    }

    static PaymentRpcException unavailable(String message) {
        return new PaymentRpcException(
            Status.Code.UNAVAILABLE,
            PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE,
            message
        );
    }
}
