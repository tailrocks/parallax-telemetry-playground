package dev.tailrocks.payment;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

class PaymentIdentityTest {
    @Test
    void authorize_identity_is_stable_for_the_tenant_and_request_key() {
        assertEquals(
            PaymentIdentity.stablePaymentId("tenant-acme", "checkout-42:authorize"),
            PaymentIdentity.stablePaymentId("tenant-acme", "checkout-42:authorize")
        );
    }

    @Test
    void authorize_identity_is_tenant_scoped() {
        assertNotEquals(
            PaymentIdentity.stablePaymentId("tenant-acme", "checkout-42:authorize"),
            PaymentIdentity.stablePaymentId("tenant-nova", "checkout-42:authorize")
        );
    }

    @Test
    void checkout_key_is_only_derived_from_the_exact_authorize_suffix() {
        assertEquals(
            "checkout-42",
            PaymentIdentity.checkoutRequestId("checkout-42:authorize").orElseThrow()
        );
        assertEquals(
            java.util.Optional.empty(),
            PaymentIdentity.checkoutRequestId("checkout-42")
        );
    }
}
