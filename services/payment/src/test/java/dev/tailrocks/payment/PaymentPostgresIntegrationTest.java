package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.AuthorizeResponse;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.Money;
import dev.tailrocks.payment.v1.PaymentMethod;
import dev.tailrocks.payment.v1.PaymentMethodType;
import dev.tailrocks.payment.v1.PaymentOperationStatus;
import dev.tailrocks.payment.v1.PaymentRecord;
import dev.tailrocks.payment.v1.PaymentFailureReason;
import dev.tailrocks.payment.v1.PaymentStatus;
import dev.tailrocks.payment.v1.RefundRequest;
import dev.tailrocks.payment.v1.RefundReason;
import java.util.UUID;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import org.springframework.mock.env.MockEnvironment;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.jdbc.core.simple.JdbcClient;
import org.springframework.jdbc.datasource.DataSourceTransactionManager;
import org.springframework.jdbc.datasource.DriverManagerDataSource;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.condition.EnabledIfEnvironmentVariable;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

/**
 * Opt-in proof that the payment lifecycle uses the shared PostgreSQL schema.
 * Set PAYMENT_INTEGRATION_DATABASE_URL to run it against a migrated database.
 */
@EnabledIfEnvironmentVariable(
    named = "PAYMENT_INTEGRATION_DATABASE_URL",
    matches = ".+"
)
class PaymentPostgresIntegrationTest {
    @Test
    void persists_and_replays_the_complete_payment_lifecycle() {
        DriverManagerDataSource dataSource = new DriverManagerDataSource(
            System.getenv("PAYMENT_INTEGRATION_DATABASE_URL"),
            envOr("PAYMENT_INTEGRATION_DATABASE_USER", "postgres"),
            envOr("PAYMENT_INTEGRATION_DATABASE_PASSWORD", "playground")
        );
        JdbcTemplate jdbc = new JdbcTemplate(dataSource);
        JdbcPaymentStore store = new JdbcPaymentStore(
            JdbcClient.create(dataSource),
            new DataSourceTransactionManager(dataSource),
            new MockEnvironment().withProperty("payment.provider", "integration")
        );

        String orderId = "payment-integration-" + UUID.randomUUID();
        String cartId = "cart-" + UUID.randomUUID();
        String requestId = "authorize-" + UUID.randomUUID();
        try {
            jdbc.update(
                "INSERT INTO carts (id, tenant_id, customer_id, session_id, status, currency, expires_at) "
                    + "VALUES (?, 'tenant-acme', 'customer-acme-ava', ?, 'active', 'USD', CURRENT_TIMESTAMP + INTERVAL '1 day')",
                cartId,
                cartId
            );
            jdbc.update(
                "INSERT INTO orders (id, tenant_id, customer_id, cart_id, order_number, status, currency, "
                    + "subtotal_minor, discount_minor, tax_minor, shipping_minor, total_minor, shipping_address, billing_address) "
                    + "VALUES (?, 'tenant-acme', 'customer-acme-ava', ?, ?, 'pending', 'USD', 2500, 0, 0, 0, 2500, '{}'::jsonb, '{}'::jsonb)",
                orderId,
                cartId,
                "PAYMENT-" + orderId.substring(orderId.length() - 12)
            );
            store.verifySharedSchema();

            AuthorizeRequest authorize = AuthorizeRequest.newBuilder()
                .setRequestId(requestId)
                .setTenantId("tenant-acme")
                .setMerchantReference(orderId)
                .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2500))
                .setPaymentMethod(PaymentMethod.newBuilder()
                    .setType(PaymentMethodType.PAYMENT_METHOD_TYPE_CARD)
                    .setToken("tok_integration"))
                .build();
            var first = store.authorize(authorize, PaymentFaultPolicy.forMethod(authorize.getPaymentMethod()));
            var replay = store.authorize(authorize, PaymentFaultPolicy.forMethod(authorize.getPaymentMethod()));
            assertEquals(first, replay);
            assertEquals(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED, first.getOperationStatus());
            assertEquals(
                1,
                jdbc.queryForObject(
                    "SELECT count(*) FROM payment_provider_authorization_decisions "
                        + "WHERE tenant_id = 'tenant-acme' AND payment_id = ? "
                        + "AND provider = 'integration' AND provider_reference = ? "
                        + "AND amount_minor = 2500 AND currency = 'USD' AND outcome = 'authorized'",
                    Integer.class,
                    first.getPayment().getPaymentId(),
                    first.getPayment().getProviderReference()
                )
            );

            var capture = store.capture(CaptureRequest.newBuilder()
                .setRequestId(requestId + ":capture")
                .setTenantId("tenant-acme")
                .setPaymentId(first.getPayment().getPaymentId())
                .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2500))
                .build());
            assertEquals(PaymentStatus.PAYMENT_STATUS_CAPTURED, capture.getPayment().getStatus());

            var refund = store.refund(RefundRequest.newBuilder()
                .setRequestId(requestId + ":refund")
                .setTenantId("tenant-acme")
                .setPaymentId(first.getPayment().getPaymentId())
                .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2500))
                .setReason(RefundReason.REFUND_REASON_ORDER_CANCELLED)
                .build());
            assertEquals(PaymentStatus.PAYMENT_STATUS_REFUNDED, refund.getPayment().getStatus());
            assertEquals(
                PaymentStatus.PAYMENT_STATUS_REFUNDED,
                store.getPayment(GetPaymentRequest.newBuilder()
                    .setTenantId("tenant-acme")
                    .setPaymentId(first.getPayment().getPaymentId())
                    .build())
                    .getStatus()
            );
        } finally {
            jdbc.update("DELETE FROM orders WHERE tenant_id = 'tenant-acme' AND id = ?", orderId);
            jdbc.update("DELETE FROM carts WHERE tenant_id = 'tenant-acme' AND id = ?", cartId);
        }
    }

    @Test
    void permits_partial_capture_and_refund_without_losing_remaining_authorization() {
        DriverManagerDataSource dataSource = new DriverManagerDataSource(
            System.getenv("PAYMENT_INTEGRATION_DATABASE_URL"),
            envOr("PAYMENT_INTEGRATION_DATABASE_USER", "postgres"),
            envOr("PAYMENT_INTEGRATION_DATABASE_PASSWORD", "playground")
        );
        JdbcTemplate jdbc = new JdbcTemplate(dataSource);
        JdbcPaymentStore store = new JdbcPaymentStore(
            JdbcClient.create(dataSource),
            new DataSourceTransactionManager(dataSource),
            new MockEnvironment().withProperty("payment.provider", "integration")
        );

        String orderId = "payment-partial-" + UUID.randomUUID();
        String cartId = "cart-" + UUID.randomUUID();
        String requestId = "authorize-" + UUID.randomUUID();
        try {
            jdbc.update(
                "INSERT INTO carts (id, tenant_id, customer_id, session_id, status, currency, expires_at) "
                    + "VALUES (?, 'tenant-acme', 'customer-acme-ava', ?, 'active', 'USD', CURRENT_TIMESTAMP + INTERVAL '1 day')",
                cartId,
                cartId
            );
            jdbc.update(
                "INSERT INTO orders (id, tenant_id, customer_id, cart_id, order_number, status, currency, "
                    + "subtotal_minor, discount_minor, tax_minor, shipping_minor, total_minor, shipping_address, billing_address) "
                    + "VALUES (?, 'tenant-acme', 'customer-acme-ava', ?, ?, 'pending', 'USD', 4000, 0, 0, 0, 4000, '{}'::jsonb, '{}'::jsonb)",
                orderId,
                cartId,
                "PAYMENT-" + orderId.substring(orderId.length() - 12)
            );

            AuthorizeRequest authorize = AuthorizeRequest.newBuilder()
                .setRequestId(requestId)
                .setTenantId("tenant-acme")
                .setMerchantReference(orderId)
                .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(4000))
                .setPaymentMethod(PaymentMethod.newBuilder()
                    .setType(PaymentMethodType.PAYMENT_METHOD_TYPE_CARD)
                    .setToken("tok_partial_integration"))
                .build();
            var authorized = store.authorize(
                authorize,
                PaymentFaultPolicy.forMethod(authorize.getPaymentMethod())
            );

            var partialCapture = store.capture(CaptureRequest.newBuilder()
                .setRequestId(requestId + ":capture-partial")
                .setTenantId("tenant-acme")
                .setPaymentId(authorized.getPayment().getPaymentId())
                .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(1000))
                .build());
            assertEquals(PaymentStatus.PAYMENT_STATUS_AUTHORIZED, partialCapture.getPayment().getStatus());
            assertEquals(1000, partialCapture.getPayment().getCapturedAmount().getAmountMinor());

            var partialRefund = store.refund(RefundRequest.newBuilder()
                .setRequestId(requestId + ":refund-partial")
                .setTenantId("tenant-acme")
                .setPaymentId(authorized.getPayment().getPaymentId())
                .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(400))
                .setReason(RefundReason.REFUND_REASON_ORDER_CANCELLED)
                .build());
            assertEquals(
                PaymentStatus.PAYMENT_STATUS_PARTIALLY_REFUNDED,
                partialRefund.getPayment().getStatus()
            );
            assertEquals(400, partialRefund.getPayment().getRefundedAmount().getAmountMinor());

            var finalCapture = store.capture(CaptureRequest.newBuilder()
                .setRequestId(requestId + ":capture-final")
                .setTenantId("tenant-acme")
                .setPaymentId(authorized.getPayment().getPaymentId())
                .build());
            assertEquals(
                PaymentStatus.PAYMENT_STATUS_PARTIALLY_REFUNDED,
                finalCapture.getPayment().getStatus()
            );
            assertEquals(4000, finalCapture.getPayment().getCapturedAmount().getAmountMinor());

            var finalRefund = store.refund(RefundRequest.newBuilder()
                .setRequestId(requestId + ":refund-final")
                .setTenantId("tenant-acme")
                .setPaymentId(authorized.getPayment().getPaymentId())
                .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(3600))
                .setReason(RefundReason.REFUND_REASON_ORDER_CANCELLED)
                .build());
            assertEquals(PaymentStatus.PAYMENT_STATUS_REFUNDED, finalRefund.getPayment().getStatus());
            assertEquals(
                PaymentStatus.PAYMENT_STATUS_REFUNDED,
                store.getPayment(GetPaymentRequest.newBuilder()
                    .setTenantId("tenant-acme")
                    .setPaymentId(authorized.getPayment().getPaymentId())
                    .build())
                    .getStatus()
            );
        } finally {
            jdbc.update("DELETE FROM orders WHERE tenant_id = 'tenant-acme' AND id = ?", orderId);
            jdbc.update("DELETE FROM carts WHERE tenant_id = 'tenant-acme' AND id = ?", cartId);
        }
    }

    @Test
    void converges_pending_provider_decisions_and_wins_reconciliation_races_once() throws Exception {
        DriverManagerDataSource dataSource = new DriverManagerDataSource(
            System.getenv("PAYMENT_INTEGRATION_DATABASE_URL"),
            envOr("PAYMENT_INTEGRATION_DATABASE_USER", "postgres"),
            envOr("PAYMENT_INTEGRATION_DATABASE_PASSWORD", "playground")
        );
        JdbcTemplate jdbc = new JdbcTemplate(dataSource);
        JdbcPaymentStore store = paymentStore(dataSource);
        String successOrderId = "payment-pending-success-" + UUID.randomUUID();
        String successCartId = "cart-" + UUID.randomUUID();
        String failedOrderId = "payment-pending-failed-" + UUID.randomUUID();
        String failedCartId = "cart-" + UUID.randomUUID();
        ExecutorService racers = Executors.newFixedThreadPool(2);
        try {
            insertOrder(jdbc, successOrderId, successCartId, 2500);
            insertOrder(jdbc, failedOrderId, failedCartId, 2600);

            AuthorizeRequest successRequest = authorizeRequest(
                "authorize-pending-success-" + UUID.randomUUID(),
                successOrderId,
                2500,
                "tok_pending"
            );
            var pendingSuccess = store.authorize(
                successRequest,
                PaymentFaultPolicy.forMethod(successRequest.getPaymentMethod())
            );
            assertEquals(
                PaymentOperationStatus.PAYMENT_OPERATION_STATUS_PENDING,
                pendingSuccess.getOperationStatus()
            );
            assertEquals(
                PaymentStatus.PAYMENT_STATUS_AUTHORIZED,
                store.getPayment(GetPaymentRequest.newBuilder()
                    .setTenantId("tenant-acme")
                    .setPaymentId(pendingSuccess.getPayment().getPaymentId())
                    .build())
                    .getStatus()
            );
            assertEquals(0, store.reconcilePendingPayments(1));
            var successReplay = store.authorize(
                successRequest,
                PaymentFaultPolicy.forMethod(successRequest.getPaymentMethod())
            );
            assertEquals(
                PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED,
                successReplay.getOperationStatus()
            );
            assertEquals(PaymentStatus.PAYMENT_STATUS_AUTHORIZED, successReplay.getPayment().getStatus());
            AuthorizeRequest failedRequest = authorizeRequest(
                "authorize-pending-failed-" + UUID.randomUUID(),
                failedOrderId,
                2600,
                "tok_pending_fail"
            );
            var pendingFailure = store.authorize(
                failedRequest,
                PaymentFaultPolicy.forMethod(failedRequest.getPaymentMethod())
            );
            String failedPaymentId = pendingFailure.getPayment().getPaymentId();
            CyclicBarrier start = new CyclicBarrier(2);
            var first = racers.submit(() -> {
                start.await();
                return store.reconcilePendingPayments(1);
            });
            var second = racers.submit(() -> {
                start.await();
                return store.reconcilePendingPayments(1);
            });
            assertEquals(1, first.get(5, TimeUnit.SECONDS) + second.get(5, TimeUnit.SECONDS));

            var failedReplay = store.authorize(
                failedRequest,
                PaymentFaultPolicy.forMethod(failedRequest.getPaymentMethod())
            );
            assertEquals(
                PaymentOperationStatus.PAYMENT_OPERATION_STATUS_FAILED,
                failedReplay.getOperationStatus()
            );
            assertEquals(PaymentStatus.PAYMENT_STATUS_FAILED, failedReplay.getPayment().getStatus());
            assertEquals(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE,
                failedReplay.getFailureReason()
            );
            assertEquals(
                1,
                jdbc.queryForObject(
                    "SELECT pending_reconciliation_attempts FROM payments WHERE tenant_id = 'tenant-acme' AND id = ?",
                    Integer.class,
                    failedPaymentId
                )
            );
            assertEquals(
                0,
                jdbc.queryForObject(
                    "SELECT count(*) FROM payment_operation_requests WHERE tenant_id = 'tenant-acme' AND request_id = ? AND encode(response_payload, 'escape') LIKE '%tok_pending_fail%'",
                    Integer.class,
                    failedRequest.getRequestId()
                )
            );
        } finally {
            racers.shutdownNow();
            jdbc.update("DELETE FROM orders WHERE tenant_id = 'tenant-acme' AND id IN (?, ?)", successOrderId, failedOrderId);
            jdbc.update("DELETE FROM carts WHERE tenant_id = 'tenant-acme' AND id IN (?, ?)", successCartId, failedCartId);
        }
    }

    @Test
    void wakes_the_checkout_reconciliation_job_after_provider_resolution() {
        DriverManagerDataSource dataSource = new DriverManagerDataSource(
            System.getenv("PAYMENT_INTEGRATION_DATABASE_URL"),
            envOr("PAYMENT_INTEGRATION_DATABASE_USER", "postgres"),
            envOr("PAYMENT_INTEGRATION_DATABASE_PASSWORD", "playground")
        );
        JdbcTemplate jdbc = new JdbcTemplate(dataSource);
        JdbcPaymentStore store = paymentStore(dataSource);
        String orderId = "payment-requeue-" + UUID.randomUUID();
        String cartId = "cart-" + UUID.randomUUID();
        String checkoutRequestId = "checkout-requeue-" + UUID.randomUUID();
        String authorizeRequestId = checkoutRequestId + ":authorize";
        try {
            insertOrder(jdbc, orderId, cartId, 2700);
            jdbc.update(
                "INSERT INTO checkout_attempts "
                    + "(tenant_id, request_id, request_fingerprint, status, order_id, response_payload, lease_token) "
                    + "VALUES ('tenant-acme', ?, 'fingerprint', 'pending', ?, '{}'::jsonb, 'lease-requeue')",
                checkoutRequestId,
                orderId
            );
            jdbc.update(
                "INSERT INTO checkout_payment_reconciliations "
                    + "(tenant_id, request_id, order_id, authorize_request_id, merchant_reference, amount_minor, "
                    + "currency, method_type, feature_variant, status) "
                    + "VALUES ('tenant-acme', ?, ?, ?, ?, 2700, 'USD', 'card', 'control', 'awaiting_provider')",
                checkoutRequestId,
                orderId,
                authorizeRequestId,
                orderId
            );

            AuthorizeRequest request = authorizeRequest(
                authorizeRequestId,
                orderId,
                2700,
                "tok_pending"
            );
            var pending = store.authorize(request, PaymentFaultPolicy.forMethod(request.getPaymentMethod()));
            String paymentId = pending.getPayment().getPaymentId();

            assertEquals(1, store.reconcilePendingPayment("tenant-acme", paymentId));
            var reconciliation = jdbc.queryForMap(
                "SELECT payment_id, status FROM checkout_payment_reconciliations "
                    + "WHERE tenant_id='tenant-acme' AND request_id=? AND authorize_request_id=?",
                checkoutRequestId,
                authorizeRequestId
            );
            assertEquals(paymentId, reconciliation.get("payment_id"));
            assertEquals("queued", reconciliation.get("status"));
        } finally {
            jdbc.update("DELETE FROM orders WHERE tenant_id = 'tenant-acme' AND id = ?", orderId);
            jdbc.update("DELETE FROM carts WHERE tenant_id = 'tenant-acme' AND id = ?", cartId);
        }
    }

    @Test
    void quarantines_legacy_unknown_outcomes_and_rejects_capture_without_proof() {
        DriverManagerDataSource dataSource = new DriverManagerDataSource(
            System.getenv("PAYMENT_INTEGRATION_DATABASE_URL"),
            envOr("PAYMENT_INTEGRATION_DATABASE_USER", "postgres"),
            envOr("PAYMENT_INTEGRATION_DATABASE_PASSWORD", "playground")
        );
        JdbcTemplate jdbc = new JdbcTemplate(dataSource);
        JdbcPaymentStore store = paymentStore(dataSource);
        String orderId = "payment-legacy-unknown-" + UUID.randomUUID();
        String cartId = "cart-" + UUID.randomUUID();
        String paymentId = "payment-legacy-" + UUID.randomUUID();
        String requestId = "authorize-legacy-" + UUID.randomUUID();
        String providerReference = "legacy-provider-" + UUID.randomUUID();
        try {
            insertOrder(jdbc, orderId, cartId, 2800);
            jdbc.update(
                "INSERT INTO payments "
                    + "(id, tenant_id, order_id, provider, provider_reference, status, amount_minor, "
                    + "currency, method_type, captured_amount_minor, refunded_amount_minor, "
                    + "pending_reconciliation_at) "
                    + "VALUES (?, 'tenant-acme', ?, 'integration', ?, 'pending', 2800, 'USD', 'card', 0, 0, "
                    + "CURRENT_TIMESTAMP - INTERVAL '1 minute')",
                paymentId,
                orderId,
                providerReference
            );
            AuthorizeResponse pendingResponse = AuthorizeResponse.newBuilder()
                .setOperationStatus(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_PENDING)
                .setPayment(PaymentRecord.newBuilder()
                    .setPaymentId(paymentId)
                    .setMerchantReference(orderId)
                    .setAuthorizedAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2800))
                    .setStatus(PaymentStatus.PAYMENT_STATUS_PENDING)
                    .setMethodType(PaymentMethodType.PAYMENT_METHOD_TYPE_CARD)
                    .setProviderReference(providerReference)
                    .setCapturedAmount(Money.newBuilder().setCurrencyCode("USD"))
                    .setRefundedAmount(Money.newBuilder().setCurrencyCode("USD"))
                    .build())
                .build();
            jdbc.update(
                "INSERT INTO payment_operation_requests "
                    + "(tenant_id, request_id, operation, request_fingerprint, payment_id, "
                    + "response_payload, operation_amount_minor) "
                    + "VALUES ('tenant-acme', ?, 'authorize', 'legacy-fingerprint', ?, ?, 2800)",
                requestId,
                paymentId,
                pendingResponse.toByteArray()
            );

            var read = store.getPayment(GetPaymentRequest.newBuilder()
                .setTenantId("tenant-acme")
                .setPaymentId(paymentId)
                .build());
            assertEquals(PaymentStatus.PAYMENT_STATUS_PENDING, read.getStatus());
            var state = jdbc.queryForMap(
                "SELECT status, pending_resolution, provider_decision_verified_at, "
                    + "pending_reconciliation_at FROM payments WHERE tenant_id='tenant-acme' AND id=?",
                paymentId
            );
            assertEquals("pending", state.get("status"));
            assertEquals("manual_reconciliation", state.get("pending_resolution"));
            assertNull(state.get("provider_decision_verified_at"));
            assertNull(state.get("pending_reconciliation_at"));

            assertThrows(
                PaymentRpcException.class,
                () -> store.capture(CaptureRequest.newBuilder()
                    .setRequestId(requestId + ":capture")
                    .setTenantId("tenant-acme")
                    .setPaymentId(paymentId)
                    .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2800))
                    .build())
            );
        } finally {
            jdbc.update("DELETE FROM orders WHERE tenant_id = 'tenant-acme' AND id = ?", orderId);
            jdbc.update("DELETE FROM carts WHERE tenant_id = 'tenant-acme' AND id = ?", cartId);
        }
    }

    @Test
    void ignores_a_legacy_proof_marker_without_an_exact_provider_decision() {
        DriverManagerDataSource dataSource = new DriverManagerDataSource(
            System.getenv("PAYMENT_INTEGRATION_DATABASE_URL"),
            envOr("PAYMENT_INTEGRATION_DATABASE_USER", "postgres"),
            envOr("PAYMENT_INTEGRATION_DATABASE_PASSWORD", "playground")
        );
        JdbcTemplate jdbc = new JdbcTemplate(dataSource);
        JdbcPaymentStore store = paymentStore(dataSource);
        String orderId = "payment-marker-only-" + UUID.randomUUID();
        String cartId = "cart-" + UUID.randomUUID();
        String paymentId = "payment-marker-only-" + UUID.randomUUID();
        String providerReference = "marker-only-provider-" + UUID.randomUUID();
        try {
            insertOrder(jdbc, orderId, cartId, 2900);
            jdbc.update(
                "INSERT INTO payments "
                    + "(id, tenant_id, order_id, provider, provider_reference, status, amount_minor, "
                    + "currency, method_type, captured_amount_minor, refunded_amount_minor, "
                    + "authorized_at, pending_resolution, pending_resolution_at, "
                    + "provider_decision_verified_at) "
                    + "VALUES (?, 'tenant-acme', ?, 'integration', ?, 'authorized', 2900, 'USD', 'card', 0, 0, "
                    + "CURRENT_TIMESTAMP, 'authorized', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
                paymentId,
                orderId,
                providerReference
            );

            var read = store.getPayment(GetPaymentRequest.newBuilder()
                .setTenantId("tenant-acme")
                .setPaymentId(paymentId)
                .build());
            assertEquals(PaymentStatus.PAYMENT_STATUS_PENDING, read.getStatus());
            assertThrows(
                PaymentRpcException.class,
                () -> store.capture(CaptureRequest.newBuilder()
                    .setRequestId("capture-marker-only-" + UUID.randomUUID())
                    .setTenantId("tenant-acme")
                    .setPaymentId(paymentId)
                    .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(2900))
                    .build())
            );
        } finally {
            jdbc.update("DELETE FROM orders WHERE tenant_id = 'tenant-acme' AND id = ?", orderId);
            jdbc.update("DELETE FROM carts WHERE tenant_id = 'tenant-acme' AND id = ?", cartId);
        }
    }

    @Test
    void does_not_reconcile_a_provider_decision_for_the_wrong_amount() {
        DriverManagerDataSource dataSource = new DriverManagerDataSource(
            System.getenv("PAYMENT_INTEGRATION_DATABASE_URL"),
            envOr("PAYMENT_INTEGRATION_DATABASE_USER", "postgres"),
            envOr("PAYMENT_INTEGRATION_DATABASE_PASSWORD", "playground")
        );
        JdbcTemplate jdbc = new JdbcTemplate(dataSource);
        JdbcPaymentStore store = paymentStore(dataSource);
        String orderId = "payment-wrong-amount-" + UUID.randomUUID();
        String cartId = "cart-" + UUID.randomUUID();
        String requestId = "authorize-wrong-amount-" + UUID.randomUUID();
        try {
            insertOrder(jdbc, orderId, cartId, 3000);
            AuthorizeRequest request = authorizeRequest(requestId, orderId, 3000, "tok_pending_unknown");
            var pending = store.authorize(
                request,
                PaymentFaultPolicy.forMethod(request.getPaymentMethod())
            );
            String paymentId = pending.getPayment().getPaymentId();
            jdbc.update(
                "INSERT INTO payment_provider_authorization_decisions "
                    + "(tenant_id, payment_id, provider, provider_reference, amount_minor, currency, outcome, verified_at) "
                    + "VALUES ('tenant-acme', ?, 'integration', ?, 3001, 'USD', 'authorized', CURRENT_TIMESTAMP)",
                paymentId,
                pending.getPayment().getProviderReference()
            );

            var read = store.getPayment(GetPaymentRequest.newBuilder()
                .setTenantId("tenant-acme")
                .setPaymentId(paymentId)
                .build());
            assertEquals(PaymentStatus.PAYMENT_STATUS_PENDING, read.getStatus());
            assertEquals(0, store.reconcilePendingPayment("tenant-acme", paymentId));
        } finally {
            jdbc.update("DELETE FROM orders WHERE tenant_id = 'tenant-acme' AND id = ?", orderId);
            jdbc.update("DELETE FROM carts WHERE tenant_id = 'tenant-acme' AND id = ?", cartId);
        }
    }

    private static JdbcPaymentStore paymentStore(DriverManagerDataSource dataSource) {
        return new JdbcPaymentStore(
            JdbcClient.create(dataSource),
            new DataSourceTransactionManager(dataSource),
            new MockEnvironment().withProperty("payment.provider", "integration")
        );
    }

    private static void insertOrder(JdbcTemplate jdbc, String orderId, String cartId, int totalMinor) {
        jdbc.update(
            "INSERT INTO carts (id, tenant_id, customer_id, session_id, status, currency, expires_at) "
                + "VALUES (?, 'tenant-acme', 'customer-acme-ava', ?, 'active', 'USD', CURRENT_TIMESTAMP + INTERVAL '1 day')",
            cartId,
            cartId
        );
        jdbc.update(
            "INSERT INTO orders (id, tenant_id, customer_id, cart_id, order_number, status, currency, "
                + "subtotal_minor, discount_minor, tax_minor, shipping_minor, total_minor, shipping_address, billing_address) "
                + "VALUES (?, 'tenant-acme', 'customer-acme-ava', ?, ?, 'pending', 'USD', ?, 0, 0, 0, ?, '{}'::jsonb, '{}'::jsonb)",
            orderId,
            cartId,
            "PAYMENT-" + orderId.substring(orderId.length() - 12),
            totalMinor,
            totalMinor
        );
    }

    private static AuthorizeRequest authorizeRequest(
        String requestId,
        String orderId,
        int amountMinor,
        String token
    ) {
        return AuthorizeRequest.newBuilder()
            .setRequestId(requestId)
            .setTenantId("tenant-acme")
            .setMerchantReference(orderId)
            .setAmount(Money.newBuilder().setCurrencyCode("USD").setAmountMinor(amountMinor))
            .setPaymentMethod(PaymentMethod.newBuilder()
                .setType(PaymentMethodType.PAYMENT_METHOD_TYPE_CARD)
                .setToken(token))
            .build();
    }

    private static String envOr(String name, String fallback) {
        String value = System.getenv(name);
        return value == null || value.isBlank() ? fallback : value;
    }
}
