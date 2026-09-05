package dev.tailrocks.payment;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Component;

/** Bounded durable provider reconciliation; multiple instances coordinate in PostgreSQL. */
@Component
class PendingPaymentReconciliationWorker {
    private static final Logger LOG = LoggerFactory.getLogger(PendingPaymentReconciliationWorker.class);

    private final PaymentStore store;
    private final int batchSize;

    PendingPaymentReconciliationWorker(
        PaymentStore store,
        @Value("${payment.reconciliation.batch-size:32}") int batchSize
    ) {
        if (batchSize <= 0) {
            throw new IllegalArgumentException("payment reconciliation batch size must be positive");
        }
        this.store = store;
        this.batchSize = batchSize;
    }

    @Scheduled(
        fixedDelayString = "${payment.reconciliation.fixed-delay-ms:250}",
        initialDelayString = "${payment.reconciliation.initial-delay-ms:100}"
    )
    void reconcileDuePayments() {
        try {
            int reconciled = store.reconcilePendingPayments(batchSize);
            if (reconciled > 0) {
                LOG.atInfo()
                    .addKeyValue("payment.reconciliation.count", reconciled)
                    .log("pending payment reconciliation completed");
            }
        } catch (RuntimeException error) {
            // A failed tick rolls back its transaction. The next bounded tick retries it.
            LOG.atWarn().setCause(error).log("pending payment reconciliation tick failed");
        }
    }
}
