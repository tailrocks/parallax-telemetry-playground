package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.AuthorizeResponse;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.CaptureResponse;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.GetPaymentResponse;
import dev.tailrocks.payment.v1.RefundRequest;
import dev.tailrocks.payment.v1.RefundResponse;
import dev.tailrocks.payment.v1.VoidRequest;
import dev.tailrocks.payment.v1.VoidResponse;

interface PaymentStore {
    AuthorizeResponse authorize(
        AuthorizeRequest request,
        PaymentFaultPolicy.Decision decision
    );

    CaptureResponse capture(CaptureRequest request);

    VoidResponse voidPayment(VoidRequest request);

    RefundResponse refund(RefundRequest request);

    GetPaymentResponse getPayment(GetPaymentRequest request);

    /** Reconcile at most maxBatch due pending authorizations. */
    int reconcilePendingPayments(int maxBatch);

    /** Reconcile one due pending authorization before a read. */
    int reconcilePendingPayment(String tenantId, String paymentId);
}
