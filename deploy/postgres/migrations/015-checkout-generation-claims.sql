-- Separate durable checkout generations from worker claims.
--
-- checkout_attempts.lease_token is the parent generation.  Reconciliation
-- workers and outbox publishers get independent claim tokens so a reclaimed
-- worker can never complete work owned by its successor.

SET TIME ZONE 'UTC';
SET search_path TO public;

-- Compensation rows are children of one checkout generation.  Older rows
-- were created before that identity was mandatory; recover their owner only
-- when exactly one checkout attempt proves the pairing, then fail closed.
ALTER TABLE checkout_compensation_tasks
    ADD COLUMN IF NOT EXISTS remote_operation_id TEXT,
    ADD COLUMN IF NOT EXISTS remote_operation_started_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS remote_operation_completed_at TIMESTAMPTZ;

WITH owner AS (
    SELECT task.id, parent.request_id, parent.lease_token
    FROM checkout_compensation_tasks task
    JOIN LATERAL (
        SELECT min(attempt.request_id) AS request_id,
               min(attempt.lease_token) AS lease_token,
               count(*) AS attempt_count
        FROM checkout_attempts attempt
        WHERE attempt.tenant_id = task.tenant_id
          AND (
              NULLIF(btrim(task.checkout_request_id), '') IS NULL
              OR attempt.request_id = task.checkout_request_id
          )
          AND attempt.order_id = task.order_id
    ) parent ON parent.attempt_count = 1
    WHERE NULLIF(btrim(task.checkout_request_id), '') IS NULL
       OR NULLIF(btrim(task.checkout_lease_token), '') IS NULL
)
UPDATE checkout_compensation_tasks task
SET checkout_request_id = COALESCE(NULLIF(btrim(task.checkout_request_id), ''), owner.request_id),
    checkout_lease_token = COALESCE(NULLIF(btrim(task.checkout_lease_token), ''), owner.lease_token)
FROM owner
WHERE task.id = owner.id;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM checkout_compensation_tasks task
        WHERE NULLIF(btrim(task.checkout_request_id), '') IS NULL
           OR NULLIF(btrim(task.checkout_lease_token), '') IS NULL
    ) THEN
        RAISE EXCEPTION
            'checkout_compensation_tasks contains a row without a checkout generation';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM checkout_compensation_tasks task
        WHERE NOT EXISTS (
            SELECT 1
            FROM checkout_attempts attempt
            WHERE attempt.tenant_id = task.tenant_id
              AND attempt.request_id = task.checkout_request_id
              AND attempt.order_id = task.order_id
              AND attempt.lease_token = task.checkout_lease_token
        )
    ) THEN
        RAISE EXCEPTION
            'checkout_compensation_tasks contains an unproven checkout generation';
    END IF;
END
$$;

ALTER TABLE checkout_compensation_tasks
    ALTER COLUMN checkout_request_id SET NOT NULL,
    ALTER COLUMN checkout_lease_token SET NOT NULL;

ALTER TABLE checkout_compensation_tasks
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_checkout_request_nonempty,
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_checkout_lease_nonempty,
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_remote_operation_check,
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_status_check;

ALTER TABLE checkout_compensation_tasks
    ADD CONSTRAINT checkout_compensation_tasks_checkout_request_nonempty
        CHECK (length(btrim(checkout_request_id)) > 0),
    ADD CONSTRAINT checkout_compensation_tasks_checkout_lease_nonempty
        CHECK (length(btrim(checkout_lease_token)) > 0),
    ADD CONSTRAINT checkout_compensation_tasks_remote_operation_check
        CHECK (
            (
                remote_operation_id IS NULL
                AND remote_operation_started_at IS NULL
                AND remote_operation_completed_at IS NULL
            )
            OR (
                length(btrim(remote_operation_id)) > 0
                AND remote_operation_started_at IS NOT NULL
                AND (
                    remote_operation_completed_at IS NULL
                    OR remote_operation_completed_at >= remote_operation_started_at
                )
            )
        ),
    ADD CONSTRAINT checkout_compensation_tasks_status_check
        CHECK (status IN ('prepared', 'queued', 'processing', 'completed', 'failed', 'superseded'));

-- An old processing row has an unknown remote outcome.  Preserve that fact
-- as an in-flight operation so a later worker retries the same idempotent
-- remote action instead of treating the row as a fresh intent.
UPDATE checkout_compensation_tasks
SET remote_operation_id = COALESCE(remote_operation_id, 'legacy-compensation:' || id),
    remote_operation_started_at = COALESCE(
        remote_operation_started_at,
        COALESCE(claimed_at, updated_at)
    )
WHERE status = 'processing';

-- Task identity includes the parent generation.  This keeps a quarantined
-- task and its successor distinct while retaining the existing unique key
-- contract on (tenant_id, task_key).
UPDATE checkout_compensation_tasks
SET task_key = task_key || ':' || checkout_lease_token
WHERE right(task_key, length(checkout_lease_token) + 1)
      <> ':' || checkout_lease_token;

ALTER TABLE checkout_payment_reconciliations
    ADD COLUMN IF NOT EXISTS checkout_lease_token TEXT;

UPDATE checkout_payment_reconciliations reconciliation
SET checkout_lease_token = attempt.lease_token
FROM checkout_attempts attempt
WHERE attempt.tenant_id = reconciliation.tenant_id
  AND attempt.request_id = reconciliation.request_id
  AND reconciliation.checkout_lease_token IS NULL;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM checkout_payment_reconciliations
        WHERE checkout_lease_token IS NULL OR length(btrim(checkout_lease_token)) = 0
    ) THEN
        RAISE EXCEPTION
            'checkout_payment_reconciliations contains a row without a parent checkout generation';
    END IF;
END
$$;

ALTER TABLE checkout_payment_reconciliations
    ALTER COLUMN checkout_lease_token SET NOT NULL;

ALTER TABLE checkout_payment_reconciliations
    DROP CONSTRAINT IF EXISTS checkout_payment_reconciliations_parent_generation_check;

ALTER TABLE checkout_payment_reconciliations
    ADD CONSTRAINT checkout_payment_reconciliations_parent_generation_check
    CHECK (length(btrim(checkout_lease_token)) > 0);

CREATE INDEX IF NOT EXISTS idx_checkout_payment_reconciliation_parent_generation
    ON checkout_payment_reconciliations (tenant_id, request_id, checkout_lease_token);

ALTER TABLE outbox_events
    ADD COLUMN IF NOT EXISTS claim_token TEXT,
    ADD COLUMN IF NOT EXISTS claim_expires_at TIMESTAMPTZ;

UPDATE outbox_events
SET claim_token = COALESCE(claim_token, md5('legacy-outbox:' || id)),
    claim_expires_at = COALESCE(claim_expires_at, available_at)
WHERE status = 'processing';

UPDATE outbox_events
SET claim_token = NULL,
    claim_expires_at = NULL
WHERE status <> 'processing';

ALTER TABLE outbox_events
    DROP CONSTRAINT IF EXISTS outbox_claim_check;

ALTER TABLE outbox_events
    ADD CONSTRAINT outbox_claim_check
    CHECK (
        (status = 'processing' AND claim_token IS NOT NULL AND claim_expires_at IS NOT NULL)
        OR (status <> 'processing' AND claim_token IS NULL AND claim_expires_at IS NULL)
    );

CREATE INDEX IF NOT EXISTS idx_outbox_claim_expiry
    ON outbox_events (claim_expires_at, created_at)
    WHERE status = 'processing';
