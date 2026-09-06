package dev.tailrocks.payment;

import dev.tailrocks.payment.v1.AuthorizeRequest;
import dev.tailrocks.payment.v1.AuthorizeResponse;
import dev.tailrocks.payment.v1.CaptureRequest;
import dev.tailrocks.payment.v1.CaptureResponse;
import dev.tailrocks.payment.v1.GetPaymentRequest;
import dev.tailrocks.payment.v1.GetPaymentResponse;
import dev.tailrocks.payment.v1.Money;
import dev.tailrocks.payment.v1.PaymentFailureReason;
import dev.tailrocks.payment.v1.PaymentMethodType;
import dev.tailrocks.payment.v1.PaymentOperationStatus;
import dev.tailrocks.payment.v1.PaymentRecord;
import dev.tailrocks.payment.v1.PaymentStatus;
import dev.tailrocks.payment.v1.RefundRequest;
import dev.tailrocks.payment.v1.RefundResponse;
import dev.tailrocks.payment.v1.VoidRequest;
import dev.tailrocks.payment.v1.VoidResponse;
import com.google.protobuf.MessageLite;
import com.google.protobuf.Parser;
import com.google.protobuf.InvalidProtocolBufferException;
import io.grpc.Status;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.sql.ResultSet;
import java.sql.SQLException;
import java.sql.Timestamp;
import java.time.Instant;
import java.util.HexFormat;
import java.util.List;
import java.util.Locale;
import java.util.Objects;
import jakarta.annotation.PostConstruct;
import org.springframework.core.env.Environment;
import org.springframework.jdbc.core.simple.JdbcClient;
import org.springframework.stereotype.Repository;
import org.springframework.transaction.PlatformTransactionManager;
import org.springframework.transaction.support.TransactionTemplate;

/** PostgreSQL-backed payment state and durable request replay ledger. */
@Repository
class JdbcPaymentStore implements PaymentStore {
    private final JdbcClient jdbc;
    private final TransactionTemplate transactions;
    private final String provider;

    JdbcPaymentStore(
        JdbcClient jdbc,
        PlatformTransactionManager transactionManager,
        Environment environment
    ) {
        this.jdbc = jdbc;
        this.transactions = new TransactionTemplate(transactionManager);
        this.provider = environment.getProperty("payment.provider", "playground");
    }

    /** Fail startup when the shared Postgres migration has not run; never fall back silently. */
    @PostConstruct
    void verifySharedSchema() {
        SharedSchema schema = jdbc.sql("""
                SELECT to_regclass('public.payments') AS payments,
                       to_regclass('public.payment_operation_requests') AS payment_operations,
                       to_regclass('public.checkout_payment_reconciliations') AS checkout_reconciliations,
                       to_regclass('public.payment_provider_authorization_decisions') AS provider_decisions,
                       (
                           SELECT count(*) = 5
                           FROM information_schema.columns
                           WHERE table_schema = 'public'
                             AND table_name = 'payments'
                             AND column_name IN (
                                 'pending_resolution',
                                 'pending_reconciliation_attempts',
                                 'pending_reconciliation_at',
                                 'pending_resolution_at',
                                 'provider_decision_verified_at'
                             )
                       ) AS pending_reconciliation_columns,
                       (
                           SELECT count(*) = 3
                           FROM information_schema.columns
                           WHERE table_schema = 'public'
                             AND table_name = 'checkout_payment_reconciliations'
                             AND column_name IN ('request_id', 'authorize_request_id', 'payment_id')
                       ) AS checkout_reconciliation_columns
                """)
            .query((resultSet, rowNumber) -> new SharedSchema(
                resultSet.getString("payments"),
                resultSet.getString("payment_operations"),
                resultSet.getString("checkout_reconciliations"),
                resultSet.getString("provider_decisions"),
                resultSet.getBoolean("pending_reconciliation_columns"),
                resultSet.getBoolean("checkout_reconciliation_columns")
            ))
            .single();
        if (schema.payments() == null
            || schema.paymentOperations() == null
            || schema.checkoutReconciliations() == null
            || schema.providerDecisions() == null
            || !schema.pendingReconciliationColumns()
            || !schema.checkoutReconciliationColumns()) {
            throw new IllegalStateException(
                "mise run infra:postgres_migrate must apply checkout migration 011 and payment migration 012 before payment starts"
            );
        }
    }

    @Override
    public AuthorizeResponse authorize(
        AuthorizeRequest request,
        PaymentFaultPolicy.Decision decision
    ) {
        return executeIdempotently(
            "authorize",
            request.getTenantId(),
            request.getRequestId(),
            request,
            AuthorizeResponse.parser(),
            null,
            () -> authorizeOnce(request, decision)
        );
    }

    @Override
    public CaptureResponse capture(CaptureRequest request) {
        return executeIdempotently(
            "capture",
            request.getTenantId(),
            request.getRequestId(),
            request,
            CaptureResponse.parser(),
            request.getPaymentId(),
            () -> captureOnce(request)
        );
    }

    @Override
    public VoidResponse voidPayment(VoidRequest request) {
        return executeIdempotently(
            "void",
            request.getTenantId(),
            request.getRequestId(),
            request,
            VoidResponse.parser(),
            request.getPaymentId(),
            () -> voidOnce(request)
        );
    }

    @Override
    public RefundResponse refund(RefundRequest request) {
        return executeIdempotently(
            "refund",
            request.getTenantId(),
            request.getRequestId(),
            request,
            RefundResponse.parser(),
            request.getPaymentId(),
            () -> refundOnce(request)
        );
    }

    @Override
    public GetPaymentResponse getPayment(GetPaymentRequest request) {
        reconcilePendingPayment(request.getTenantId(), request.getPaymentId());
        PaymentRow row = findPayment(request.getTenantId(), request.getPaymentId(), false);
        if (row == null) {
            throw PaymentRpcException.notFound("payment was not found");
        }
        PaymentRecord payment = toRecord(row);
        return GetPaymentResponse.newBuilder()
            .setPayment(payment)
            .setStatus(payment.getStatus())
            .build();
    }

    @Override
    public int reconcilePendingPayments(int maxBatch) {
        if (maxBatch <= 0) {
            throw new IllegalArgumentException("payment reconciliation batch size must be positive");
        }
        int boundedBatch = Math.min(maxBatch, 100);
        Integer reconciled = transactions.execute(status -> {
            List<PendingPayment> candidates = jdbc.sql("""
                    SELECT p.tenant_id, p.id, por.request_id, p.pending_resolution,
                           p.provider, p.provider_reference, p.amount_minor, p.currency
                    FROM payments p
                    JOIN payment_operation_requests por
                      ON por.tenant_id = p.tenant_id AND por.payment_id = p.id
                     AND por.operation = 'authorize'
                    WHERE p.status = 'pending'
                      AND (
                          p.pending_reconciliation_at <= CURRENT_TIMESTAMP
                          OR EXISTS (
                              SELECT 1
                              FROM payment_provider_authorization_decisions d
                              WHERE d.tenant_id = p.tenant_id
                                AND d.payment_id = p.id
                                AND d.provider = p.provider
                                AND d.provider_reference = p.provider_reference
                                AND d.amount_minor = p.amount_minor
                                AND d.currency = p.currency
                                AND d.verified_at IS NOT NULL
                          )
                      )
                    ORDER BY p.pending_reconciliation_at, p.created_at, p.id
                    LIMIT :limit
                    FOR UPDATE OF por SKIP LOCKED
                    """)
                .param("limit", boundedBatch)
                .query(JdbcPaymentStore::mapPendingPayment)
                .list();
            int count = 0;
            for (PendingPayment candidate : candidates) {
                PaymentRow row = findPayment(candidate.tenantId(), candidate.paymentId(), true);
                if (row != null && "pending".equals(row.databaseStatus())) {
                    count += reconcilePending(candidate, row);
                }
            }
            return count;
        });
        return Objects.requireNonNull(reconciled, "payment reconciliation transaction returned no count");
    }

    @Override
    public int reconcilePendingPayment(String tenantId, String paymentId) {
        Integer reconciled = transactions.execute(status -> {
            PendingPayment candidate = findPendingPayment(tenantId, paymentId, false);
            if (candidate == null) {
                return 0;
            }
            PaymentRow row = findPayment(tenantId, paymentId, true);
            return row == null ? 0 : reconcilePending(candidate, row);
        });
        return Objects.requireNonNull(reconciled, "payment reconciliation transaction returned no count");
    }

    private OperationResult<AuthorizeResponse> authorizeOnce(
        AuthorizeRequest request,
        PaymentFaultPolicy.Decision decision
    ) {
        String tenantId = request.getTenantId();
        OrderRow order = lockOrder(tenantId, request.getMerchantReference());
        if (!"pending".equals(order.status()) && !"confirmed".equals(order.status())) {
            throw PaymentRpcException.failedPrecondition("order is not payable");
        }
        if (order.totalMinor() != request.getAmount().getAmountMinor()) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                "amount.amount_minor must match the order total"
            );
        }
        if (!order.currency().equals(request.getAmount().getCurrencyCode())) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                "amount.currency_code must match the order currency"
            );
        }
        PaymentRow existing = findByMerchantReference(tenantId, request.getMerchantReference(), true);
        if (existing != null) {
            throw PaymentRpcException.alreadyExists("order already has an active payment");
        }

        String paymentId = PaymentIdentity.stablePaymentId(tenantId, request.getRequestId());
        String providerReference = PaymentIdentity.stableProviderReference(provider, paymentId);
        String databaseStatus = switch (decision.kind()) {
            case NONE -> "authorized";
            case PENDING -> "pending";
            case DECLINED, INSUFFICIENT_FUNDS, INVALID_PAYMENT_METHOD -> "failed";
            case UNAVAILABLE, DEADLINE_EXCEEDED, INTERNAL ->
                throw new IllegalStateException("transport fault must not reach the store");
        };
        String pendingResolution = decision.kind() == PaymentFaultPolicy.Kind.PENDING
            ? Objects.requireNonNull(decision.pendingResolution(), "pending resolution is required")
                .databaseValue()
            : decision.kind() == PaymentFaultPolicy.Kind.NONE ? "authorized" : null;
        boolean providerDecisionAvailable = decision.kind() == PaymentFaultPolicy.Kind.NONE
            || decision.kind() == PaymentFaultPolicy.Kind.PENDING
                && decision.pendingResolution() != null
                && decision.pendingResolution().hasProviderDecision();
        String providerDecisionOutcome = decision.kind() == PaymentFaultPolicy.Kind.NONE
            ? "authorized"
            : providerDecisionAvailable
                ? Objects.requireNonNull(decision.pendingResolution(), "pending resolution is required")
                    .databaseValue()
                : null;
        Timestamp providerDecisionVerifiedAt = providerDecisionAvailable
            ? Timestamp.from(Instant.now())
            : null;
        boolean manualReconciliation = "manual_reconciliation".equals(pendingResolution);
        String failureCode = manualReconciliation
            ? "provider_outcome_unknown"
            : decision.failureReason()
                == PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED
                ? null
                : decision.failureReason().name().toLowerCase(Locale.ROOT);
        PaymentFailureReason operationFailureReason = manualReconciliation
            ? PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE
            : decision.failureReason();
        Timestamp authorizedAt = "authorized".equals(databaseStatus)
            ? providerDecisionVerifiedAt
            : null;
        Timestamp pendingReconciliationAt = decision.kind() == PaymentFaultPolicy.Kind.PENDING
            && providerDecisionAvailable
            ? providerDecisionVerifiedAt
            : null;
        Timestamp pendingResolutionAt = "authorized".equals(databaseStatus)
            ? providerDecisionVerifiedAt
            : null;
        int amountMinor = Math.toIntExact(request.getAmount().getAmountMinor());

        jdbc.sql("""
                INSERT INTO payments (
                    id, tenant_id, order_id, provider, provider_reference, status,
                    amount_minor, currency, failure_code, authorized_at, method_type,
                    captured_amount_minor, refunded_amount_minor,
                    pending_resolution, pending_reconciliation_at,
                    pending_resolution_at,
                    provider_decision_verified_at
                ) VALUES (
                    :id, :tenant_id, :order_id, :provider, :provider_reference, :status,
                    :amount_minor, :currency, :failure_code, :authorized_at, :method_type,
                    0, 0, :pending_resolution,
                    :pending_reconciliation_at,
                    :pending_resolution_at,
                    :provider_decision_verified_at
                )
                """)
            .param("id", paymentId)
            .param("tenant_id", tenantId)
            .param("order_id", request.getMerchantReference())
            .param("provider", provider)
            .param("provider_reference", providerReference)
            .param("status", databaseStatus)
            .param("amount_minor", amountMinor)
            .param("currency", request.getAmount().getCurrencyCode())
            .param("failure_code", failureCode)
            .param("authorized_at", authorizedAt)
            .param("method_type", toDatabaseMethodType(request.getPaymentMethod().getType()))
            .param("pending_resolution", pendingResolution)
            .param("pending_reconciliation_at", pendingReconciliationAt)
            .param("pending_resolution_at", pendingResolutionAt)
            .param("provider_decision_verified_at", authorizedAt)
            .update();

        if (providerDecisionAvailable) {
            jdbc.sql("""
                    INSERT INTO payment_provider_authorization_decisions (
                        tenant_id, payment_id, provider, provider_reference,
                        amount_minor, currency, outcome, verified_at
                    ) VALUES (
                        :tenant_id, :payment_id, :provider, :provider_reference,
                        :amount_minor, :currency, :outcome, :verified_at
                    )
                    """)
                .param("tenant_id", tenantId)
                .param("payment_id", paymentId)
                .param("provider", provider)
                .param("provider_reference", providerReference)
                .param("amount_minor", amountMinor)
                .param("currency", request.getAmount().getCurrencyCode())
                .param("outcome", providerDecisionOutcome)
                .param("verified_at", providerDecisionVerifiedAt)
                .update();
        }

        PaymentRow row = new PaymentRow(
            paymentId,
            request.getMerchantReference(),
            providerReference,
            databaseStatus,
            failureCode,
            request.getAmount().getAmountMinor(),
            request.getAmount().getCurrencyCode(),
            toDatabaseMethodType(request.getPaymentMethod().getType()),
            0,
            0,
            authorizedAt
        );
        PaymentRecord payment = toRecord(row);
        AuthorizeResponse response = AuthorizeResponse.newBuilder()
            .setOperationStatus(decision.operationStatus())
            .setPayment(payment)
            .setFailureReason(operationFailureReason)
            .build();
        return new OperationResult<>(response, request.getAmount().getAmountMinor());
    }

    private OperationResult<CaptureResponse> captureOnce(CaptureRequest request) {
        PaymentRow row = findPayment(request.getTenantId(), request.getPaymentId(), true);
        if (row == null) {
            throw PaymentRpcException.notFound("payment was not found");
        }
        if (!isCaptureable(row)) {
            throw PaymentRpcException.failedPrecondition("payment is not authorized");
        }
        if (request.hasAmount()
            && !row.currency().equals(request.getAmount().getCurrencyCode())) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                "amount.currency_code must match the authorization"
            );
        }

        long remaining = row.authorizedAmountMinor() - row.capturedAmountMinor();
        long amount = request.hasAmount() ? request.getAmount().getAmountMinor() : remaining;
        if (amount <= 0 || amount > remaining) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                "capture amount exceeds the uncaptured authorization"
            );
        }
        long captured = row.capturedAmountMinor() + amount;
        long refunded = row.refundedAmountMinor();
        String databaseStatus = statusForBalances(row.authorizedAmountMinor(), captured, refunded);
        jdbc.sql("""
                UPDATE payments
                SET status = :status,
                    captured_amount_minor = :captured_amount_minor,
                    captured_at = CASE
                        WHEN :captured_amount_minor = :authorized_amount_minor
                            THEN COALESCE(captured_at, CURRENT_TIMESTAMP)
                        ELSE captured_at
                    END
                WHERE tenant_id = :tenant_id AND id = :id
                """)
            .param("status", databaseStatus)
            .param("captured_amount_minor", Math.toIntExact(captured))
            .param("authorized_amount_minor", Math.toIntExact(row.authorizedAmountMinor()))
            .param("tenant_id", request.getTenantId())
            .param("id", row.paymentId())
            .update();

        PaymentRecord payment = toRecord(row.withStatus(databaseStatus).withCaptured(captured));
        CaptureResponse response = CaptureResponse.newBuilder()
            .setOperationStatus(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED)
            .setPayment(payment)
            .setFailureReason(PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED)
            .build();
        return new OperationResult<>(response, amount);
    }

    private OperationResult<VoidResponse> voidOnce(VoidRequest request) {
        PaymentRow row = findPayment(request.getTenantId(), request.getPaymentId(), true);
        if (row == null) {
            throw PaymentRpcException.notFound("payment was not found");
        }
        if (!"authorized".equals(row.databaseStatus())
            || !row.hasProviderDecisionProof()
            || row.capturedAmountMinor() != 0) {
            throw PaymentRpcException.failedPrecondition("payment authorization is not voidable");
        }
        jdbc.sql("""
                UPDATE payments
                SET status = 'voided'
                WHERE tenant_id = :tenant_id AND id = :id
                """)
            .param("tenant_id", request.getTenantId())
            .param("id", row.paymentId())
            .update();

        PaymentRecord payment = toRecord(row.withStatus("voided"));
        VoidResponse response = VoidResponse.newBuilder()
            .setOperationStatus(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED)
            .setPayment(payment)
            .setFailureReason(PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED)
            .build();
        return new OperationResult<>(response, 0);
    }

    private OperationResult<RefundResponse> refundOnce(RefundRequest request) {
        PaymentRow row = findPayment(request.getTenantId(), request.getPaymentId(), true);
        if (row == null) {
            throw PaymentRpcException.notFound("payment was not found");
        }
        long refundable = row.capturedAmountMinor() - row.refundedAmountMinor();
        if (!isRefundable(row) || refundable <= 0) {
            throw PaymentRpcException.failedPrecondition("payment has no captured funds");
        }
        if (!row.currency().equals(request.getAmount().getCurrencyCode())) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                "amount.currency_code must match the authorization"
            );
        }
        long amount = request.getAmount().getAmountMinor();
        if (amount <= 0 || amount > refundable) {
            throw PaymentRpcException.invalidArgument(
                PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED,
                "refund amount exceeds captured funds"
            );
        }
        long refunded = row.refundedAmountMinor() + amount;
        String databaseStatus = statusForBalances(
            row.authorizedAmountMinor(),
            row.capturedAmountMinor(),
            refunded
        );
        jdbc.sql("""
                UPDATE payments
                SET status = :status, refunded_amount_minor = :refunded_amount_minor
                WHERE tenant_id = :tenant_id AND id = :id
                """)
            .param("status", databaseStatus)
            .param("refunded_amount_minor", Math.toIntExact(refunded))
            .param("tenant_id", request.getTenantId())
            .param("id", row.paymentId())
            .update();

        PaymentRecord payment = toRecord(row.withStatus(databaseStatus).withRefunded(refunded));
        RefundResponse response = RefundResponse.newBuilder()
            .setOperationStatus(PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED)
            .setPayment(payment)
            .setFailureReason(PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED)
            .build();
        return new OperationResult<>(response, amount);
    }

    private <T extends MessageLite> T executeIdempotently(
        String operation,
        String tenantId,
        String requestId,
        MessageLite request,
        Parser<T> parser,
        String paymentId,
        DbOperation<T> action
    ) {
        TxOutcome<T> outcome = transactions.execute(status -> {
            lockRequest(tenantId, requestId);
            String fingerprint = fingerprint(request);
            StoredOperation stored = findStoredOperation(tenantId, requestId, true);
            if (stored != null) {
                if (!stored.operation().equals(operation)
                    || !stored.requestFingerprint().equals(fingerprint)) {
                    throw PaymentRpcException.alreadyExists(
                        "request_id was already used for a different payment operation"
                    );
                }
                if (stored.isPendingAuthorize()) {
                    reconcileStoredPending(tenantId, stored);
                    stored = findStoredOperation(tenantId, requestId, true);
                    if (stored == null) {
                        throw new IllegalStateException("stored payment operation disappeared");
                    }
                }
                return stored.replay(parser);
            }

            try {
                OperationResult<T> result = action.run();
                insertStoredSuccess(
                    tenantId,
                    requestId,
                    operation,
                    fingerprint,
                    paymentIdFor(result.response(), paymentId),
                    result
                );
                return new TxOutcome<>(result.response(), null);
            } catch (PaymentRpcException error) {
                insertStoredFailure(
                    tenantId,
                    requestId,
                    operation,
                    fingerprint,
                    paymentId,
                    error
                );
                return new TxOutcome<>(null, error);
            }
        });
        TxOutcome<T> nonNullOutcome = Objects.requireNonNull(
            outcome,
            "payment transaction returned no outcome"
        );
        if (nonNullOutcome.error() != null) {
            throw nonNullOutcome.error();
        }
        return Objects.requireNonNull(nonNullOutcome.response(), "stored response was empty");
    }

    private void reconcileStoredPending(String tenantId, StoredOperation stored) {
        if (stored.paymentId() == null) {
            return;
        }
        PendingPayment pending = findPendingPayment(tenantId, stored.paymentId(), false);
        if (pending == null) {
            return;
        }
        PaymentRow row = findPayment(tenantId, stored.paymentId(), true);
        if (row != null) {
            reconcilePending(pending, row);
        }
    }

    private int reconcilePending(PendingPayment pending, PaymentRow row) {
        if (!"pending".equals(row.databaseStatus())) {
            return 0;
        }
        ProviderDecision providerDecision = lookupProviderDecision(pending);
        if (providerDecision == null) {
            quarantinePending(pending);
            return 0;
        }

        String status;
        String failureCode;
        PaymentOperationStatus operationStatus;
        PaymentFailureReason failureReason;
        switch (providerDecision.outcome()) {
            case "authorized" -> {
                status = "authorized";
                failureCode = null;
                operationStatus = PaymentOperationStatus.PAYMENT_OPERATION_STATUS_SUCCEEDED;
                failureReason = PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED;
            }
            case "failed" -> {
                status = "failed";
                failureCode = PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE
                    .name()
                    .toLowerCase(Locale.ROOT);
                operationStatus = PaymentOperationStatus.PAYMENT_OPERATION_STATUS_FAILED;
                failureReason = PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE;
            }
            default -> throw new IllegalStateException(
                "unsupported provider payment decision: " + providerDecision.outcome()
            );
        }

        int updated = jdbc.sql("""
                UPDATE payments
                SET status = :status,
                    failure_code = :failure_code,
                    authorized_at = CASE
                        WHEN :status = 'authorized' THEN COALESCE(authorized_at, :verified_at)
                        ELSE authorized_at
                    END,
                    provider_decision_verified_at = :verified_at,
                    pending_resolution = :pending_resolution,
                    pending_reconciliation_attempts = pending_reconciliation_attempts + 1,
                    pending_reconciliation_at = NULL,
                    pending_resolution_at = CURRENT_TIMESTAMP
                WHERE tenant_id = :tenant_id
                  AND id = :id
                  AND status = 'pending'
                """)
            .param("status", status)
            .param("failure_code", failureCode)
            .param("verified_at", providerDecision.verifiedAt())
            .param("pending_resolution", providerDecision.outcome())
            .param("tenant_id", pending.tenantId())
            .param("id", pending.paymentId())
            .update();
        if (updated == 0) {
            return 0;
        }

        PaymentRow resolved = row.withStatus(status)
            .withFailureCode(failureCode)
            .withProviderDecisionVerifiedAt(providerDecision.verifiedAt());
        AuthorizeResponse response = AuthorizeResponse.newBuilder()
            .setOperationStatus(operationStatus)
            .setPayment(toRecord(resolved))
            .setFailureReason(failureReason)
            .build();
        int operationUpdated = jdbc.sql("""
                UPDATE payment_operation_requests
                SET response_payload = :response_payload,
                    operation_amount_minor = :operation_amount_minor
                WHERE tenant_id = :tenant_id
                  AND request_id = :request_id
                  AND operation = 'authorize'
                  AND response_payload IS NOT NULL
                """)
            .param("response_payload", response.toByteArray())
            .param("operation_amount_minor", pending.amountMinor())
            .param("tenant_id", pending.tenantId())
            .param("request_id", pending.requestId())
            .update();
        if (operationUpdated != 1) {
            throw new IllegalStateException(
                "pending payment has no stored authorize response: " + pending.paymentId()
            );
        }
        requeueCheckoutReconciliation(pending, resolved.paymentId());
        return 1;
    }

    private ProviderDecision lookupProviderDecision(PendingPayment pending) {
        return jdbc.sql("""
                SELECT d.outcome, d.verified_at
                FROM payment_provider_authorization_decisions d
                WHERE d.tenant_id = :tenant_id
                  AND d.payment_id = :payment_id
                  AND d.provider = :provider
                  AND d.provider_reference = :provider_reference
                  AND d.amount_minor = :amount_minor
                  AND d.currency = :currency
                  AND d.verified_at IS NOT NULL
                ORDER BY d.verified_at DESC
                LIMIT 1
                """)
            .param("tenant_id", pending.tenantId())
            .param("payment_id", pending.paymentId())
            .param("provider", pending.provider())
            .param("provider_reference", pending.providerReference())
            .param("amount_minor", Math.toIntExact(pending.amountMinor()))
            .param("currency", pending.currency())
            .query((resultSet, rowNumber) -> new ProviderDecision(
                resultSet.getString("outcome"),
                resultSet.getTimestamp("verified_at")
            ))
            .optional()
            .orElse(null);
    }

    private void quarantinePending(PendingPayment pending) {
        jdbc.sql("""
                UPDATE payments
                SET pending_resolution = 'manual_reconciliation',
                    failure_code = 'provider_outcome_unknown',
                    pending_reconciliation_at = NULL,
                    pending_resolution_at = NULL,
                    provider_decision_verified_at = NULL
                WHERE tenant_id = :tenant_id
                  AND id = :id
                  AND status = 'pending'
                """)
            .param("tenant_id", pending.tenantId())
            .param("id", pending.paymentId())
            .update();
    }

    private void requeueCheckoutReconciliation(PendingPayment pending, String paymentId) {
        PaymentIdentity.checkoutRequestId(pending.requestId()).ifPresent(checkoutRequestId -> {
            int changed = jdbc.sql("""
                    UPDATE checkout_payment_reconciliations
                    SET payment_id = COALESCE(payment_id, :payment_id),
                        status = 'queued',
                        attempts = 0,
                        available_at = CURRENT_TIMESTAMP,
                        claimed_at = NULL,
                        lease_token = NULL,
                        last_error = NULL,
                        updated_at = CURRENT_TIMESTAMP
                    WHERE tenant_id = :tenant_id
                      AND request_id = :checkout_request_id
                      AND authorize_request_id = :authorize_request_id
                      AND status IN ('awaiting_provider', 'queued')
                      AND (payment_id IS NULL OR payment_id = :payment_id)
                    """)
                .param("tenant_id", pending.tenantId())
                .param("checkout_request_id", checkoutRequestId)
                .param("authorize_request_id", pending.requestId())
                .param("payment_id", paymentId)
                .update();
            if (changed == 0) {
                String conflictingPaymentId = jdbc.sql("""
                        SELECT payment_id
                        FROM checkout_payment_reconciliations
                        WHERE tenant_id = :tenant_id
                          AND request_id = :checkout_request_id
                          AND authorize_request_id = :authorize_request_id
                          AND status IN ('awaiting_provider', 'queued')
                        """)
                    .param("tenant_id", pending.tenantId())
                    .param("checkout_request_id", checkoutRequestId)
                    .param("authorize_request_id", pending.requestId())
                    .query((resultSet, rowNumber) -> resultSet.getString("payment_id"))
                    .optional()
                    .orElse(null);
                if (conflictingPaymentId != null && !conflictingPaymentId.equals(paymentId)) {
                    throw new IllegalStateException(
                        "checkout payment reconciliation references a different payment"
                    );
                }
            }
        });
    }

    private void lockRequest(String tenantId, String requestId) {
        jdbc.sql("SELECT pg_advisory_xact_lock(hashtextextended(:lock_key, 0))")
            .param("lock_key", "payment-request:" + tenantId + ":" + requestId)
            // pg_advisory_xact_lock returns PostgreSQL's void type. Consume the
            // row without asking the JDBC driver to coerce it to a scalar.
            .query((resultSet, rowNumber) -> Boolean.TRUE)
            .single();
    }

    private OrderRow lockOrder(String tenantId, String orderId) {
        return jdbc.sql("""
                SELECT status, total_minor, currency
                FROM orders
                WHERE tenant_id = :tenant_id AND id = :order_id
                FOR UPDATE
                """)
            .param("tenant_id", tenantId)
            .param("order_id", orderId)
            .query((resultSet, rowNumber) -> new OrderRow(
                resultSet.getString("status"),
                resultSet.getLong("total_minor"),
                resultSet.getString("currency")
            ))
            .optional()
            .orElseThrow(() -> PaymentRpcException.notFound("merchant reference order was not found"));
    }

    private StoredOperation findStoredOperation(String tenantId, String requestId, boolean forUpdate) {
        String lock = forUpdate ? " FOR UPDATE" : "";
        return jdbc.sql("""
                SELECT request_id, operation, request_fingerprint, payment_id,
                       response_payload, grpc_status, failure_reason, error_message
                FROM payment_operation_requests
                WHERE tenant_id = :tenant_id AND request_id = :request_id
                """ + lock)
            .param("tenant_id", tenantId)
            .param("request_id", requestId)
            .query(JdbcPaymentStore::mapStoredOperation)
            .optional()
            .orElse(null);
    }

    private PendingPayment findPendingPayment(
        String tenantId,
        String paymentId,
        boolean skipLocked
    ) {
        String lock = skipLocked
            ? " FOR UPDATE OF por SKIP LOCKED"
            : " FOR UPDATE OF por";
        return jdbc.sql("""
                SELECT p.tenant_id, p.id, por.request_id, p.pending_resolution,
                       p.provider, p.provider_reference, p.amount_minor, p.currency
                FROM payments p
                JOIN payment_operation_requests por
                  ON por.tenant_id = p.tenant_id AND por.payment_id = p.id
                 AND por.operation = 'authorize'
                WHERE p.tenant_id = :tenant_id
                  AND p.id = :payment_id
                  AND p.status = 'pending'
                  AND (
                      p.pending_reconciliation_at <= CURRENT_TIMESTAMP
                      OR p.pending_resolution = 'manual_reconciliation'
                      OR EXISTS (
                          SELECT 1
                          FROM payment_provider_authorization_decisions d
                          WHERE d.tenant_id = p.tenant_id
                            AND d.payment_id = p.id
                            AND d.provider = p.provider
                            AND d.provider_reference = p.provider_reference
                            AND d.amount_minor = p.amount_minor
                            AND d.currency = p.currency
                            AND d.verified_at IS NOT NULL
                      )
                  )
                LIMIT 1
                """ + lock)
            .param("tenant_id", tenantId)
            .param("payment_id", paymentId)
            .query(JdbcPaymentStore::mapPendingPayment)
            .optional()
            .orElse(null);
    }

    private void insertStoredSuccess(
        String tenantId,
        String requestId,
        String operation,
        String fingerprint,
        String paymentId,
        OperationResult<? extends MessageLite> result
    ) {
        jdbc.sql("""
                INSERT INTO payment_operation_requests (
                    tenant_id, request_id, operation, request_fingerprint, payment_id,
                    response_payload, operation_amount_minor
                ) VALUES (
                    :tenant_id, :request_id, :operation, :request_fingerprint, :payment_id,
                    :response_payload, :operation_amount_minor
                )
                """)
            .param("tenant_id", tenantId)
            .param("request_id", requestId)
            .param("operation", operation)
            .param("request_fingerprint", fingerprint)
            .param("payment_id", paymentId)
            .param("response_payload", result.response().toByteArray())
            .param("operation_amount_minor", result.amountMinor())
            .update();
    }

    private void insertStoredFailure(
        String tenantId,
        String requestId,
        String operation,
        String fingerprint,
        String paymentId,
        PaymentRpcException error
    ) {
        jdbc.sql("""
                INSERT INTO payment_operation_requests (
                    tenant_id, request_id, operation, request_fingerprint, payment_id,
                    grpc_status, failure_reason, error_message
                ) VALUES (
                    :tenant_id, :request_id, :operation, :request_fingerprint, :payment_id,
                    :grpc_status, :failure_reason, :error_message
                )
                """)
            .param("tenant_id", tenantId)
            .param("request_id", requestId)
            .param("operation", operation)
            .param("request_fingerprint", fingerprint)
            .param("payment_id", paymentId)
            .param("grpc_status", error.statusCode().name())
            .param("failure_reason", error.failureReason().name())
            .param("error_message", error.getMessage())
            .update();
    }

    private static String paymentIdFor(MessageLite response, String paymentId) {
        if (paymentId != null) {
            return paymentId;
        }
        if (response instanceof AuthorizeResponse authorize && authorize.hasPayment()) {
            return authorize.getPayment().getPaymentId();
        }
        return null;
    }

    private PaymentRow findPayment(String tenantId, String paymentId, boolean forUpdate) {
        String lock = forUpdate ? " FOR UPDATE" : "";
        return jdbc.sql("""
                SELECT id, order_id, provider_reference, status, amount_minor, currency,
                       failure_code, method_type, captured_amount_minor, refunded_amount_minor,
                       (
                           SELECT d.verified_at
                           FROM payment_provider_authorization_decisions d
                           WHERE d.tenant_id = payments.tenant_id
                             AND d.payment_id = payments.id
                             AND d.provider = payments.provider
                             AND d.provider_reference = payments.provider_reference
                             AND d.amount_minor = payments.amount_minor
                             AND d.currency = payments.currency
                             AND d.outcome = 'authorized'
                             AND d.verified_at IS NOT NULL
                           ORDER BY d.verified_at DESC
                           LIMIT 1
                       ) AS provider_decision_verified_at
                FROM payments
                WHERE tenant_id = :tenant_id AND id = :id
                """ + lock)
            .param("tenant_id", tenantId)
            .param("id", paymentId)
            .query(JdbcPaymentStore::mapPayment)
            .optional()
            .orElse(null);
    }

    private PaymentRow findByMerchantReference(String tenantId, String merchantReference, boolean forUpdate) {
        String lock = forUpdate ? " FOR UPDATE" : "";
        return jdbc.sql("""
                SELECT id, order_id, provider_reference, status, amount_minor, currency,
                       failure_code, method_type, captured_amount_minor, refunded_amount_minor,
                       (
                           SELECT d.verified_at
                           FROM payment_provider_authorization_decisions d
                           WHERE d.tenant_id = payments.tenant_id
                             AND d.payment_id = payments.id
                             AND d.provider = payments.provider
                             AND d.provider_reference = payments.provider_reference
                             AND d.amount_minor = payments.amount_minor
                             AND d.currency = payments.currency
                             AND d.outcome = 'authorized'
                             AND d.verified_at IS NOT NULL
                           ORDER BY d.verified_at DESC
                           LIMIT 1
                       ) AS provider_decision_verified_at
                FROM payments
                WHERE tenant_id = :tenant_id AND order_id = :order_id
                  AND (
                      status IN ('pending', 'authorized', 'captured', 'partially_refunded')
                      OR (status = 'refunded' AND captured_amount_minor < amount_minor)
                  )
                ORDER BY created_at DESC
                LIMIT 1
                """ + lock)
            .param("tenant_id", tenantId)
            .param("order_id", merchantReference)
            .query(JdbcPaymentStore::mapPayment)
            .optional()
            .orElse(null);
    }

    private PaymentRecord toRecord(PaymentRow row) {
        return PaymentRecord.newBuilder()
            .setPaymentId(row.paymentId())
            .setMerchantReference(row.merchantReference())
            .setAuthorizedAmount(money(row.currency(), row.authorizedAmountMinor()))
            .setStatus(toPaymentStatus(row))
            .setMethodType(fromDatabaseMethodType(row.methodType()))
            .setFailureReason(toFailureReason(row))
            .setProviderReference(Objects.requireNonNullElse(row.providerReference(), ""))
            .setCapturedAmount(money(row.currency(), row.capturedAmountMinor()))
            .setRefundedAmount(money(row.currency(), row.refundedAmountMinor()))
            .build();
    }

    private static PaymentStatus toPaymentStatus(PaymentRow row) {
        return switch (row.databaseStatus()) {
            case "pending" -> PaymentStatus.PAYMENT_STATUS_PENDING;
            case "voided" -> PaymentStatus.PAYMENT_STATUS_VOIDED;
            case "failed" -> PaymentStatus.PAYMENT_STATUS_FAILED;
            case "authorized", "captured", "partially_refunded", "refunded" ->
                row.hasProviderDecisionProof()
                    ? balancesStatus(row)
                    : PaymentStatus.PAYMENT_STATUS_PENDING;
            default -> PaymentStatus.PAYMENT_STATUS_UNSPECIFIED;
        };
    }

    private static PaymentFailureReason toFailureReason(PaymentRow row) {
        if (!row.hasProviderDecisionProof()
            && switch (row.databaseStatus()) {
                case "authorized", "captured", "partially_refunded", "refunded" -> true;
                default -> false;
            }) {
            return PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE;
        }
        return fromFailureCode(row.failureCode());
    }

    private static boolean isCaptureable(PaymentRow row) {
        if (row.capturedAmountMinor() >= row.authorizedAmountMinor()) {
            return false;
        }
        return row.hasProviderDecisionProof() && switch (row.databaseStatus()) {
            case "authorized", "partially_refunded", "refunded" -> true;
            default -> false;
        };
    }

    private static boolean isRefundable(PaymentRow row) {
        return row.hasProviderDecisionProof() && switch (row.databaseStatus()) {
            case "authorized", "captured", "partially_refunded", "refunded" -> true;
            default -> false;
        };
    }

    private static String statusForBalances(long authorized, long captured, long refunded) {
        if (captured < authorized) {
            if (refunded == captured && captured > 0) {
                return "refunded";
            }
            if (refunded > 0) {
                return "partially_refunded";
            }
            return "authorized";
        }
        if (refunded == 0) {
            return "captured";
        }
        return refunded == captured ? "refunded" : "partially_refunded";
    }

    private static PaymentStatus balancesStatus(PaymentRow row) {
        String status = statusForBalances(
            row.authorizedAmountMinor(),
            row.capturedAmountMinor(),
            row.refundedAmountMinor()
        );
        return switch (status) {
            case "authorized" -> PaymentStatus.PAYMENT_STATUS_AUTHORIZED;
            case "captured" -> PaymentStatus.PAYMENT_STATUS_CAPTURED;
            case "partially_refunded" -> PaymentStatus.PAYMENT_STATUS_PARTIALLY_REFUNDED;
            case "refunded" -> PaymentStatus.PAYMENT_STATUS_REFUNDED;
            default -> PaymentStatus.PAYMENT_STATUS_UNSPECIFIED;
        };
    }

    private static Money money(String currency, long amountMinor) {
        return Money.newBuilder()
            .setCurrencyCode(currency)
            .setAmountMinor(amountMinor)
            .build();
    }

    private static String toDatabaseMethodType(PaymentMethodType methodType) {
        return switch (methodType) {
            case PAYMENT_METHOD_TYPE_CARD -> "card";
            case PAYMENT_METHOD_TYPE_BANK_ACCOUNT -> "bank_account";
            case PAYMENT_METHOD_TYPE_WALLET -> "wallet";
            default -> "unspecified";
        };
    }

    private static PaymentMethodType fromDatabaseMethodType(String methodType) {
        if (methodType == null) {
            return PaymentMethodType.PAYMENT_METHOD_TYPE_UNSPECIFIED;
        }
        return switch (methodType.toLowerCase(Locale.ROOT)) {
            case "card" -> PaymentMethodType.PAYMENT_METHOD_TYPE_CARD;
            case "bank_account" -> PaymentMethodType.PAYMENT_METHOD_TYPE_BANK_ACCOUNT;
            case "wallet" -> PaymentMethodType.PAYMENT_METHOD_TYPE_WALLET;
            default -> PaymentMethodType.PAYMENT_METHOD_TYPE_UNSPECIFIED;
        };
    }

    private static PaymentFailureReason fromFailureCode(String failureCode) {
        if (failureCode == null || failureCode.isBlank()) {
            return PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED;
        }
        if ("provider_outcome_unknown".equals(failureCode)) {
            return PaymentFailureReason.PAYMENT_FAILURE_REASON_PROVIDER_UNAVAILABLE;
        }
        try {
            return PaymentFailureReason.valueOf(failureCode.toUpperCase(Locale.ROOT));
        } catch (IllegalArgumentException error) {
            return PaymentFailureReason.PAYMENT_FAILURE_REASON_UNSPECIFIED;
        }
    }

    private static PaymentRow mapPayment(ResultSet resultSet, int rowNumber) throws SQLException {
        return new PaymentRow(
            resultSet.getString("id"),
            resultSet.getString("order_id"),
            resultSet.getString("provider_reference"),
            resultSet.getString("status"),
            resultSet.getString("failure_code"),
            resultSet.getLong("amount_minor"),
            resultSet.getString("currency"),
            resultSet.getString("method_type"),
            resultSet.getLong("captured_amount_minor"),
            resultSet.getLong("refunded_amount_minor"),
            resultSet.getTimestamp("provider_decision_verified_at")
        );
    }

    private static PendingPayment mapPendingPayment(
        ResultSet resultSet,
        int rowNumber
    ) throws SQLException {
        return new PendingPayment(
            resultSet.getString("tenant_id"),
            resultSet.getString("id"),
            resultSet.getString("request_id"),
            resultSet.getString("pending_resolution"),
            resultSet.getString("provider"),
            resultSet.getString("provider_reference"),
            resultSet.getLong("amount_minor"),
            resultSet.getString("currency")
        );
    }

    private static StoredOperation mapStoredOperation(
        ResultSet resultSet,
        int rowNumber
    ) throws SQLException {
        return new StoredOperation(
            resultSet.getString("operation"),
            resultSet.getString("request_fingerprint"),
            resultSet.getString("payment_id"),
            resultSet.getBytes("response_payload"),
            resultSet.getString("grpc_status"),
            resultSet.getString("failure_reason"),
            resultSet.getString("error_message")
        );
    }

    private static String fingerprint(MessageLite request) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            return HexFormat.of().formatHex(digest.digest(request.toByteArray()));
        } catch (NoSuchAlgorithmException error) {
            throw new IllegalStateException("SHA-256 is required by the JDK", error);
        }
    }

    private record OrderRow(String status, long totalMinor, String currency) {}

    private record PaymentRow(
        String paymentId,
        String merchantReference,
        String providerReference,
        String databaseStatus,
        String failureCode,
        long authorizedAmountMinor,
        String currency,
        String methodType,
        long capturedAmountMinor,
        long refundedAmountMinor,
        Timestamp providerDecisionVerifiedAt
    ) {
        boolean hasProviderDecisionProof() {
            return providerDecisionVerifiedAt != null;
        }

        PaymentRow withStatus(String status) {
            return new PaymentRow(
                paymentId,
                merchantReference,
                providerReference,
                status,
                failureCode,
                authorizedAmountMinor,
                currency,
                methodType,
                capturedAmountMinor,
                refundedAmountMinor,
                providerDecisionVerifiedAt
            );
        }

        PaymentRow withFailureCode(String code) {
            return new PaymentRow(
                paymentId,
                merchantReference,
                providerReference,
                databaseStatus,
                code,
                authorizedAmountMinor,
                currency,
                methodType,
                capturedAmountMinor,
                refundedAmountMinor,
                providerDecisionVerifiedAt
            );
        }

        PaymentRow withCaptured(long captured) {
            return new PaymentRow(
                paymentId,
                merchantReference,
                providerReference,
                databaseStatus,
                failureCode,
                authorizedAmountMinor,
                currency,
                methodType,
                captured,
                refundedAmountMinor,
                providerDecisionVerifiedAt
            );
        }

        PaymentRow withRefunded(long refunded) {
            return new PaymentRow(
                paymentId,
                merchantReference,
                providerReference,
                databaseStatus,
                failureCode,
                authorizedAmountMinor,
                currency,
                methodType,
                capturedAmountMinor,
                refunded,
                providerDecisionVerifiedAt
            );
        }

        PaymentRow withProviderDecisionVerifiedAt(Timestamp verifiedAt) {
            return new PaymentRow(
                paymentId,
                merchantReference,
                providerReference,
                databaseStatus,
                failureCode,
                authorizedAmountMinor,
                currency,
                methodType,
                capturedAmountMinor,
                refundedAmountMinor,
                verifiedAt
            );
        }
    }

    private record OperationResult<T extends MessageLite>(T response, long amountMinor) {}

    private record TxOutcome<T extends MessageLite>(T response, PaymentRpcException error) {}

    private record SharedSchema(
        String payments,
        String paymentOperations,
        String checkoutReconciliations,
        String providerDecisions,
        boolean pendingReconciliationColumns,
        boolean checkoutReconciliationColumns
    ) {}

    private record PendingPayment(
        String tenantId,
        String paymentId,
        String requestId,
        String pendingResolution,
        String provider,
        String providerReference,
        long amountMinor,
        String currency
    ) {}

    private record ProviderDecision(String outcome, Timestamp verifiedAt) {}

    @FunctionalInterface
    private interface DbOperation<T extends MessageLite> {
        OperationResult<T> run();
    }

    private record StoredOperation(
        String operation,
        String requestFingerprint,
        String paymentId,
        byte[] responsePayload,
        String grpcStatus,
        String failureReason,
        String errorMessage
    ) {
        boolean isPendingAuthorize() {
            if (!"authorize".equals(operation) || responsePayload == null) {
                return false;
            }
            try {
                AuthorizeResponse response = AuthorizeResponse.parseFrom(responsePayload);
                return response.getOperationStatus()
                    == PaymentOperationStatus.PAYMENT_OPERATION_STATUS_PENDING;
            } catch (InvalidProtocolBufferException error) {
                throw new IllegalStateException("stored payment response is corrupt", error);
            }
        }

        <T extends MessageLite> TxOutcome<T> replay(Parser<T> parser) {
            if (responsePayload != null) {
                try {
                    return new TxOutcome<>(parser.parseFrom(responsePayload), null);
                } catch (InvalidProtocolBufferException error) {
                    throw new IllegalStateException("stored payment response is corrupt", error);
                }
            }
            PaymentFailureReason reason = PaymentFailureReason.valueOf(failureReason);
            Status.Code status = Status.Code.valueOf(grpcStatus);
            return new TxOutcome<>(
                null,
                new PaymentRpcException(status, reason, errorMessage)
            );
        }
    }
}
