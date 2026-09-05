-- Durable notification queue and channel acknowledgement state.
-- The API only enqueues. A worker owns dispatch, retries, and terminal
-- dead-letter transitions after a concrete channel acknowledgement.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE notification_deliveries
    ADD COLUMN IF NOT EXISTS next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ADD COLUMN IF NOT EXISTS lease_until TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS lease_token TEXT,
    ADD COLUMN IF NOT EXISTS last_error TEXT,
    ADD COLUMN IF NOT EXISTS dead_lettered_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS acknowledged_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ADD COLUMN IF NOT EXISTS traceparent TEXT,
    ADD COLUMN IF NOT EXISTS tracestate TEXT,
    ADD COLUMN IF NOT EXISTS baggage TEXT;

UPDATE notification_deliveries
SET next_attempt_at = COALESCE(next_attempt_at, created_at, CURRENT_TIMESTAMP),
    updated_at = COALESCE(updated_at, created_at, CURRENT_TIMESTAMP)
WHERE next_attempt_at IS NULL
   OR updated_at IS NULL;

ALTER TABLE notification_deliveries
    DROP CONSTRAINT IF EXISTS notification_deliveries_status_check;

ALTER TABLE notification_deliveries
    ADD CONSTRAINT notification_deliveries_status_check
    CHECK (status IN ('accepted', 'queued', 'processing', 'delivered', 'failed', 'dead_lettered'));

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'notification_deliveries_delivered_at_check'
    ) THEN
        ALTER TABLE notification_deliveries
            ADD CONSTRAINT notification_deliveries_delivered_at_check
            CHECK (status <> 'delivered' OR delivered_at IS NOT NULL);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'notification_deliveries_dead_lettered_at_check'
    ) THEN
        ALTER TABLE notification_deliveries
            ADD CONSTRAINT notification_deliveries_dead_lettered_at_check
            CHECK (status <> 'dead_lettered' OR dead_lettered_at IS NOT NULL);
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'notification_deliveries_lease_check'
    ) THEN
        ALTER TABLE notification_deliveries
            ADD CONSTRAINT notification_deliveries_lease_check
            CHECK (
                (status = 'processing' AND lease_until IS NOT NULL AND lease_token IS NOT NULL)
                OR status <> 'processing'
            );
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_dispatch_queue
    ON notification_deliveries (next_attempt_at, created_at, id)
    WHERE status IN ('accepted', 'queued');

CREATE INDEX IF NOT EXISTS idx_notification_deliveries_expired_leases
    ON notification_deliveries (lease_until, updated_at, id)
    WHERE status = 'processing';

CREATE TABLE IF NOT EXISTS notification_delivery_attempts (
    id BIGSERIAL PRIMARY KEY,
    delivery_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    attempt INTEGER NOT NULL CHECK (attempt > 0),
    outcome TEXT NOT NULL CHECK (outcome IN ('processing', 'delivered', 'retry', 'dead_lettered')),
    error TEXT,
    started_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at TIMESTAMPTZ,
    UNIQUE (delivery_id, attempt),
    FOREIGN KEY (tenant_id, delivery_id)
        REFERENCES notification_deliveries (tenant_id, id)
        ON DELETE CASCADE,
    CHECK (outcome = 'processing' OR completed_at IS NOT NULL)
);

CREATE INDEX IF NOT EXISTS idx_notification_delivery_attempts_tenant
    ON notification_delivery_attempts (tenant_id, delivery_id, attempt DESC);

-- This is the durable channel sink. In-app dispatch is acknowledged by this
-- row; external adapters write the same row only after their HTTP/SMTP-style
-- endpoint acknowledges the message. It makes the distinction between queue
-- acceptance and channel delivery queryable after a worker crash.
CREATE TABLE IF NOT EXISTS notification_channel_messages (
    delivery_id TEXT PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    order_id TEXT NOT NULL,
    channel TEXT NOT NULL CHECK (channel IN ('email', 'webhook', 'in_app')),
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object'),
    dispatched_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    acknowledged_at TIMESTAMPTZ NOT NULL,
    acknowledgement_reference TEXT NOT NULL,
    UNIQUE (tenant_id, delivery_id),
    FOREIGN KEY (tenant_id, delivery_id)
        REFERENCES notification_deliveries (tenant_id, id)
        ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, order_id)
        REFERENCES orders (tenant_id, id)
        ON DELETE CASCADE
);

