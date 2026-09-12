package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.PaymentFailureReason;
import dev.tailrocks.payment.v1.PaymentMethod;
import dev.tailrocks.payment.v1.PaymentMethodType;
import dev.tailrocks.payment.v1.RefundRequest;
import dev.tailrocks.payment.v1.VoidRequest;
import dev.tailrocks.payment.v1.Money;
import java.util.regex.Pattern;

final class PaymentRequestValidator {
    private static final Pattern CURRENCY = Pattern.compile("[A-Z]{3}");
    // The shared commerce schema stores amount_minor as PostgreSQL INTEGER.
    private static final long MAX_AMOUNT_MINOR = Integer.MAX_VALUE;

    private PaymentRequestValidator() {}

    static void authorize(AuthorizeRequest request) {
        requireRequestId(request.getRequestId());
        requireTenantId(request.getTenantId());
        requireText(request.getMerchantReference(), "merchant_reference", 256);
        requireMoney(request.getAmount(), "amount");
        requireMethod(request.getPaymentMethod());
    }

    static void capture(CaptureRequest request) {
        requireRequestId(request.getRequestId());
        requireTenantId(request.getTenantId());
        requireText(request.getPaymentId(), "payment_id", 128);
        if (request.hasAmount()) {
            requireMoney(request.getAmount(), "amount");
        }
    }

    static void voidPayment(VoidRequest request) {
        requireRequestId(request.getRequestId());
        requireTenantId(request.getTenantId());
        requireText(request.getPaymentId(), "payment_id", 128);
        if (request.getReason().name().endsWith("UNSPECIFIED")
            || request.getReason().name().equals("UNRECOGNIZED")) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_INVALID_STATE,
                "reason is required"
            );
        }
    }

    static void refund(RefundRequest request) {
        requireRequestId(request.getRequestId());
        requireTenantId(request.getTenantId());
        requireText(request.getPaymentId(), "payment_id", 128);
        requireMoney(request.getAmount(), "amount");
        if (request.getReason().name().endsWith("UNSPECIFIED")
            || request.getReason().name().equals("UNRECOGNIZED")) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_INVALID_STATE,
                "reason is required"
            );
        }
    }

    static void getPayment(GetPaymentRequest request) {
        requireTenantId(request.getTenantId());
        requireText(request.getPaymentId(), "payment_id", 128);
    }

    private static void requireTenantId(String tenantId) {
        requireText(tenantId, "tenant_id", 128);
    }

    private static void requireRequestId(String requestId) {
        requireText(requestId, "request_id", 128);
    }

    private static void requireMethod(PaymentMethod method) {
        if (method.getType() == PaymentMethodType.PAYMENT_METHOD_TYPE_UNSPECIFIED
            || method.getType() == PaymentMethodType.UNRECOGNIZED) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_INVALID_PAYMENT_METHOD,
                "payment_method.type is required"
            );
        }
        requireText(method.getToken(), "payment_method.token", 512);
    }

    private static void requireMoney(Money money, String field) {
        requireText(money.getCurrencyCode(), field + ".currency_code", 3);
        if (!CURRENCY.matcher(money.getCurrencyCode()).matches()) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                field + ".currency_code must be an uppercase ISO 4217 code"
            );
        }
        if (money.getAmountMinor() <= 0 || money.getAmountMinor() > MAX_AMOUNT_MINOR) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                field + ".amount_minor must be positive"
            );
        }
    }

    private static void requireText(String value, String field, int maxLength) {
        if (value == null || value.isBlank()) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                field + " is required"
            );
        }
        if (value.length() > maxLength) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                field + " exceeds maximum length"
            );
        }
    }
}
