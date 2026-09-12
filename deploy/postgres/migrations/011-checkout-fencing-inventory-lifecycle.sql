-- Fence checkout attempts and make pending payment/inventory lifecycles
-- durable.  This migration is additive; existing rows receive safe owners
-- and a bounded reservation hold.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE orders
    ADD COLUMN IF NOT EXISTS promotion_code TEXT;

ALTER TABLE checkout_attempts
    ADD COLUMN IF NOT EXISTS lease_token TEXT;

ALTER TABLE checkout_attempts
    ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ;

UPDATE checkout_attempts
SET lease_token = md5(tenant_id || ':' || request_id || ':' || created_at::TEXT)
WHERE lease_token IS NULL;

UPDATE checkout_attempts
SET lease_expires_at = COALESCE(lease_expires_at, updated_at + INTERVAL '10 minutes')
WHERE status = 'started';

ALTER TABLE checkout_attempts
    ALTER COLUMN lease_expires_at SET DEFAULT CURRENT_TIMESTAMP + INTERVAL '10 minutes';

ALTER TABLE checkout_attempts
    ALTER COLUMN lease_token SET NOT NULL;

ALTER TABLE checkout_attempts
    DROP CONSTRAINT IF EXISTS checkout_attempts_lease_token_nonempty;

ALTER TABLE checkout_attempts
    ADD CONSTRAINT checkout_attempts_lease_token_nonempty
    CHECK (length(btrim(lease_token)) > 0);

ALTER TABLE checkout_attempts
    DROP CONSTRAINT IF EXISTS checkout_attempts_lease_expiry_check;

ALTER TABLE checkout_attempts
    ADD CONSTRAINT checkout_attempts_lease_expiry_check
    CHECK (status <> 'started' OR lease_expires_at IS NOT NULL);

CREATE INDEX IF NOT EXISTS idx_checkout_attempts_lease
    ON checkout_attempts (tenant_id, request_id, lease_token, status);

ALTER TABLE inventory_reservations
    ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;

ALTER TABLE inventory_reservations
    ADD COLUMN IF NOT EXISTS consumed_at TIMESTAMPTZ;

ALTER TABLE inventory_reservations
    ADD COLUMN IF NOT EXISTS owner_request_id TEXT;

ALTER TABLE inventory_reservations
    ADD COLUMN IF NOT EXISTS owner_lease_token TEXT;

UPDATE inventory_reservations
SET expires_at = COALESCE(expires_at, updated_at + INTERVAL '15 minutes')
WHERE expires_at IS NULL;

ALTER TABLE inventory_reservations
    ALTER COLUMN expires_at SET DEFAULT CURRENT_TIMESTAMP + INTERVAL '15 minutes';

ALTER TABLE inventory_reservations
    ALTER COLUMN expires_at SET NOT NULL;

ALTER TABLE inventory_reservations
    DROP CONSTRAINT IF EXISTS inventory_reservations_status_check;

ALTER TABLE inventory_reservations
    DROP CONSTRAINT IF EXISTS inventory_reservations_check;

ALTER TABLE inventory_reservations
    ADD CONSTRAINT inventory_reservations_status_check
    CHECK (status IN ('reserved', 'released', 'consumed'));

ALTER TABLE inventory_reservations
    ADD CONSTRAINT inventory_reservations_lifecycle_check
    CHECK (
        (status = 'reserved' AND released_at IS NULL AND consumed_at IS NULL)
        OR (status = 'released' AND released_at IS NOT NULL AND consumed_at IS NULL)
        OR (status = 'consumed' AND released_at IS NULL AND consumed_at IS NOT NULL)
    );

CREATE INDEX IF NOT EXISTS idx_inventory_reservations_expiry
    ON inventory_reservations (expires_at, tenant_id, reservation_id)
    WHERE status = 'reserved';

CREATE INDEX IF NOT EXISTS idx_inventory_reservations_owner
    ON inventory_reservations (tenant_id, owner_request_id, owner_lease_token);

ALTER TABLE checkout_compensation_tasks
    ADD COLUMN IF NOT EXISTS checkout_request_id TEXT;

ALTER TABLE checkout_compensation_tasks
    ADD COLUMN IF NOT EXISTS checkout_lease_token TEXT;

-- A compensation task owns its worker claim independently from the checkout
-- lease.  The checkout token remains the remote-operation fence; retaining it
-- lets a stale task safely reconcile an already-created reservation instead
-- of failing merely because a later checkout attempt reclaimed the request.
ALTER TABLE checkout_compensation_tasks
    ADD COLUMN IF NOT EXISTS claim_token TEXT;

ALTER TABLE checkout_compensation_tasks
    ADD COLUMN IF NOT EXISTS claim_expires_at TIMESTAMPTZ;

ALTER TABLE checkout_compensation_tasks
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_status_check;

ALTER TABLE checkout_compensation_tasks
    ADD CONSTRAINT checkout_compensation_tasks_status_check
    CHECK (status IN ('prepared', 'queued', 'processing', 'completed', 'failed'));

UPDATE checkout_compensation_tasks
SET claim_token = COALESCE(claim_token, md5('legacy-compensation:' || id)),
    claim_expires_at = COALESCE(
        claim_expires_at,
        COALESCE(claimed_at, updated_at) + INTERVAL '5 minutes'
    )
WHERE status = 'processing';

UPDATE checkout_compensation_tasks
SET claim_token = NULL,
    claim_expires_at = NULL
WHERE status <> 'processing';

ALTER TABLE checkout_compensation_tasks
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_claim_check;

ALTER TABLE checkout_compensation_tasks
    ADD CONSTRAINT checkout_compensation_tasks_claim_check
    CHECK (
        (status = 'processing' AND claim_token IS NOT NULL AND claim_expires_at IS NOT NULL)
        OR (status <> 'processing' AND claim_token IS NULL AND claim_expires_at IS NULL)
    );

-- An unknown reserve outcome is a valid durable intent even when the remote
-- reservation row was never committed.  Tenant and order identity remain
-- enforced by the task itself; the reservation FK would incorrectly discard
-- the only recovery record in that case.
ALTER TABLE checkout_compensation_tasks
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_reservation_fk;

CREATE INDEX IF NOT EXISTS idx_checkout_compensation_fence
    ON checkout_compensation_tasks (tenant_id, checkout_request_id, checkout_lease_token)
    WHERE checkout_request_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_checkout_compensation_claim
    ON checkout_compensation_tasks (claim_expires_at, available_at, created_at)
    WHERE status IN ('prepared', 'queued', 'processing');

CREATE TABLE IF NOT EXISTS checkout_payment_reconciliations (
    tenant_id TEXT NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    request_id TEXT NOT NULL,
    order_id TEXT NOT NULL,
    authorize_request_id TEXT NOT NULL,
    payment_id TEXT,
    merchant_reference TEXT NOT NULL,
    amount_minor BIGINT NOT NULL CHECK (amount_minor >= 0),
    currency TEXT NOT NULL
        CHECK (char_length(currency) = 3 AND currency = upper(currency)),
    method_type TEXT NOT NULL
        CHECK (method_type IN ('card', 'bank_account', 'wallet', 'unspecified')),
    feature_variant TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('queued', 'processing', 'awaiting_provider', 'completed', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    available_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    claimed_at TIMESTAMPTZ,
    lease_token TEXT,
    lease_expires_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    last_payment_status TEXT,
    last_operation_status TEXT,
    last_failure_reason TEXT,
    last_error TEXT,
    traceparent TEXT,
    tracestate TEXT,
    baggage TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (tenant_id, request_id),
    UNIQUE (tenant_id, authorize_request_id),
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, payment_id)
        REFERENCES payments (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (
        (status = 'completed' AND completed_at IS NOT NULL)
        OR (status <> 'completed' AND completed_at IS NULL)
    ),
    CHECK (
        (status = 'processing' AND lease_token IS NOT NULL AND claimed_at IS NOT NULL)
        OR (status <> 'processing' AND lease_token IS NULL AND claimed_at IS NULL)
    )
);

ALTER TABLE checkout_payment_reconciliations
    ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ;

UPDATE checkout_payment_reconciliations
SET lease_expires_at = COALESCE(
    lease_expires_at,
    COALESCE(claimed_at, updated_at) + INTERVAL '5 minutes'
)
WHERE status = 'processing';

ALTER TABLE checkout_payment_reconciliations
    DROP CONSTRAINT IF EXISTS checkout_payment_reconciliations_lease_check;

ALTER TABLE checkout_payment_reconciliations
    ADD CONSTRAINT checkout_payment_reconciliations_lease_check
    CHECK (
        (status = 'processing' AND lease_expires_at IS NOT NULL)
        OR (status <> 'processing' AND lease_expires_at IS NULL)
    );

CREATE INDEX IF NOT EXISTS idx_checkout_payment_reconciliation_queue
    ON checkout_payment_reconciliations (available_at, updated_at)
    WHERE status IN ('queued', 'processing');

CREATE INDEX IF NOT EXISTS idx_checkout_payment_reconciliation_payment
    ON checkout_payment_reconciliations (tenant_id, payment_id, status);
