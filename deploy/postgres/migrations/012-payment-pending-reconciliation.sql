-- Persist provider reconciliation state without manufacturing an authorization.
-- Pending rows without an independently lookupable provider decision remain
-- quarantined for manual reconciliation. Payment instrument tokens are never
-- copied into reconciliation state.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE payments
    ADD COLUMN IF NOT EXISTS pending_resolution TEXT,
    ADD COLUMN IF NOT EXISTS pending_reconciliation_attempts INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS pending_reconciliation_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS pending_resolution_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS provider_decision_verified_at TIMESTAMPTZ;

-- This table is the provider adapter's lookupable decision state. The payment
-- service may resolve a pending payment only after an exact tenant/payment/
-- provider-reference/amount lookup returns one of these verified decisions.
CREATE TABLE IF NOT EXISTS payment_provider_authorization_decisions (
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    payment_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    provider_reference TEXT NOT NULL,
    amount_minor INTEGER NOT NULL CHECK (amount_minor >= 0),
    currency TEXT NOT NULL
        CHECK (char_length(currency) = 3 AND currency = upper(currency)),
    outcome TEXT NOT NULL CHECK (outcome IN ('authorized', 'failed')),
    verified_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, payment_id),
    UNIQUE (tenant_id, provider, provider_reference),
    FOREIGN KEY (tenant_id, payment_id)
        REFERENCES payments (tenant_id, id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_payment_provider_decisions_lookup
    ON payment_provider_authorization_decisions (
        tenant_id, provider, provider_reference, verified_at DESC
    );

-- A predecessor migration used the same constraint names. Remove its weaker
-- lifecycle before converting unresolved rows, then install the fail-closed
-- rules below.
ALTER TABLE payments
    DROP CONSTRAINT IF EXISTS payments_pending_resolution_value_check,
    DROP CONSTRAINT IF EXISTS payments_pending_reconciliation_state_check,
    DROP CONSTRAINT IF EXISTS payments_pending_resolution_timestamp_check,
    DROP CONSTRAINT IF EXISTS payments_pending_reconciliation_attempts_check,
    DROP CONSTRAINT IF EXISTS payments_pending_reconciliation_lifecycle_check;

UPDATE payments
SET pending_reconciliation_attempts = 0
WHERE pending_reconciliation_attempts IS NULL;

ALTER TABLE payments
    ALTER COLUMN pending_reconciliation_attempts SET DEFAULT 0,
    ALTER COLUMN pending_reconciliation_attempts SET NOT NULL;

-- Preserve only decisions backed by the lookup table. Rows with an exact
-- provider decision are schedulable; the worker still performs the same exact
-- lookup before changing status. Every other legacy pending payment becomes
-- explicit manual-reconciliation state. No row is promoted to authorized by
-- migration.
UPDATE payments AS p
SET pending_resolution = d.outcome,
    pending_reconciliation_at = COALESCE(p.pending_reconciliation_at, CURRENT_TIMESTAMP),
    pending_resolution_at = NULL,
    provider_decision_verified_at = NULL
FROM payment_provider_authorization_decisions AS d
WHERE p.status = 'pending'
  AND d.tenant_id = p.tenant_id
  AND d.payment_id = p.id
  AND d.provider = p.provider
  AND d.provider_reference = p.provider_reference
  AND d.amount_minor = p.amount_minor
  AND d.currency = p.currency
  AND d.verified_at IS NOT NULL;

UPDATE payments AS p
SET pending_resolution = 'manual_reconciliation',
    failure_code = 'provider_outcome_unknown',
    pending_reconciliation_at = NULL,
    pending_resolution_at = NULL,
    provider_decision_verified_at = NULL
WHERE p.status = 'pending'
  AND NOT EXISTS (
      SELECT 1
      FROM payment_provider_authorization_decisions AS d
      WHERE d.tenant_id = p.tenant_id
        AND d.payment_id = p.id
        AND d.provider = p.provider
        AND d.provider_reference = p.provider_reference
        AND d.amount_minor = p.amount_minor
        AND d.currency = p.currency
        AND d.verified_at IS NOT NULL
  );

-- A predecessor could have stamped a non-pending row as authorized without a
-- provider lookup record. Remove that false proof and leave the durable row in
-- a fail-closed state. If an exact decision exists, use its verification time
-- as the only accepted proof.
UPDATE payments AS p
SET pending_resolution = d.outcome,
    pending_reconciliation_at = NULL,
    pending_resolution_at = COALESCE(p.pending_resolution_at, d.verified_at),
    provider_decision_verified_at = d.verified_at
FROM payment_provider_authorization_decisions AS d
WHERE p.status <> 'pending'
  AND d.tenant_id = p.tenant_id
  AND d.payment_id = p.id
  AND d.provider = p.provider
  AND d.provider_reference = p.provider_reference
  AND d.amount_minor = p.amount_minor
  AND d.currency = p.currency
  AND d.verified_at IS NOT NULL;

UPDATE payments AS p
SET pending_resolution = NULL,
    pending_reconciliation_at = NULL,
    pending_resolution_at = NULL,
    provider_decision_verified_at = NULL
WHERE p.status <> 'pending'
  AND (p.pending_resolution IS NOT NULL OR p.provider_decision_verified_at IS NOT NULL)
  AND NOT EXISTS (
      SELECT 1
      FROM payment_provider_authorization_decisions AS d
      WHERE d.tenant_id = p.tenant_id
        AND d.payment_id = p.id
        AND d.provider = p.provider
        AND d.provider_reference = p.provider_reference
        AND d.amount_minor = p.amount_minor
        AND d.currency = p.currency
        AND d.verified_at IS NOT NULL
  );

UPDATE payments
SET pending_reconciliation_at = NULL,
    pending_resolution_at = CASE
        WHEN pending_resolution IS NULL THEN NULL
        ELSE COALESCE(pending_resolution_at, CURRENT_TIMESTAMP)
    END
WHERE status <> 'pending';

ALTER TABLE payments
    ADD CONSTRAINT payments_pending_resolution_value_check
    CHECK (
        pending_resolution IS NULL
        OR pending_resolution IN ('authorized', 'failed', 'manual_reconciliation')
    ),
    ADD CONSTRAINT payments_pending_resolution_timestamp_check
    CHECK (
        pending_resolution_at IS NULL
        OR pending_resolution IN ('authorized', 'failed')
    ),
    ADD CONSTRAINT payments_pending_reconciliation_attempts_check
    CHECK (pending_reconciliation_attempts >= 0),
    ADD CONSTRAINT payments_pending_reconciliation_state_check
    CHECK (
        status <> 'pending'
        OR (
            pending_resolution = 'manual_reconciliation'
            AND pending_reconciliation_at IS NULL
            AND pending_resolution_at IS NULL
            AND provider_decision_verified_at IS NULL
        )
        OR (
            pending_resolution IN ('authorized', 'failed')
            AND pending_reconciliation_at IS NOT NULL
            AND pending_resolution_at IS NULL
            AND provider_decision_verified_at IS NULL
        )
    ),
    ADD CONSTRAINT payments_pending_reconciliation_lifecycle_check
    CHECK (
        (
            status = 'pending'
            AND (
                (
                    pending_resolution = 'manual_reconciliation'
                    AND pending_reconciliation_at IS NULL
                    AND pending_resolution_at IS NULL
                    AND provider_decision_verified_at IS NULL
                )
                OR (
                    pending_resolution IN ('authorized', 'failed')
                    AND pending_reconciliation_at IS NOT NULL
                    AND pending_resolution_at IS NULL
                    AND provider_decision_verified_at IS NULL
                )
            )
        )
        OR (
            status <> 'pending'
            AND pending_resolution IS NULL
            AND pending_reconciliation_at IS NULL
            AND pending_resolution_at IS NULL
            AND provider_decision_verified_at IS NULL
        )
        OR (
            status <> 'pending'
            AND pending_resolution IN ('authorized', 'failed')
            AND pending_reconciliation_at IS NULL
            AND pending_resolution_at IS NOT NULL
            AND provider_decision_verified_at IS NOT NULL
        )
    );

CREATE INDEX IF NOT EXISTS idx_payments_pending_reconciliation
    ON payments (pending_reconciliation_at, tenant_id, id)
    WHERE status = 'pending';
