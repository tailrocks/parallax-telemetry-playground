-- Make exhausted transactional-outbox publishes observable and replayable.
-- A failed row is the durable logical dead-letter record. It stays in
-- outbox_events with its immutable event payload and W3C carrier. Operators
-- can inspect outbox_dead_letters and deliberately requeue one row with
-- replay_failed_outbox_event(tenant_id, event_id); replay starts a new bounded
-- three-attempt publish budget.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE outbox_events
    ADD COLUMN IF NOT EXISTS failure_code TEXT,
    ADD COLUMN IF NOT EXISTS failure_message TEXT,
    ADD COLUMN IF NOT EXISTS failed_at TIMESTAMPTZ;

-- Preserve any terminal rows created before recovery metadata existed.
UPDATE outbox_events
SET failure_code = COALESCE(failure_code, 'publish_failed'),
    failure_message = COALESCE(failure_message, 'outbox event failed before recovery metadata was added'),
    failed_at = COALESCE(failed_at, CURRENT_TIMESTAMP)
WHERE status = 'failed';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conrelid = 'public.outbox_events'::regclass
          AND conname = 'outbox_failed_metadata_check'
    ) THEN
        ALTER TABLE outbox_events
            ADD CONSTRAINT outbox_failed_metadata_check
            CHECK (
                status <> 'failed'
                OR (failed_at IS NOT NULL AND failure_code IS NOT NULL AND failure_message IS NOT NULL)
            );
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS idx_outbox_dead_letters
    ON outbox_events (failed_at, tenant_id, id)
    WHERE status = 'failed';

CREATE OR REPLACE VIEW outbox_dead_letters AS
SELECT id,
       tenant_id,
       event_key,
       aggregate_type,
       aggregate_id,
       event_type,
       schema_version,
       occurred_at,
       payload,
       traceparent,
       tracestate,
       baggage,
       attempts,
       failure_code,
       failure_message,
       failed_at,
       created_at
FROM outbox_events
WHERE status = 'failed';

CREATE OR REPLACE FUNCTION replay_failed_outbox_event(
    p_tenant_id TEXT,
    p_event_id TEXT
)
RETURNS BOOLEAN
LANGUAGE SQL
VOLATILE
AS $$
    WITH replayed AS (
        UPDATE outbox_events
        SET status = 'queued',
            attempts = 0,
            available_at = CURRENT_TIMESTAMP,
            published_at = NULL,
            failure_code = NULL,
            failure_message = NULL,
            failed_at = NULL
        WHERE tenant_id = p_tenant_id
          AND id = p_event_id
          AND status = 'failed'
        RETURNING 1
    )
    SELECT EXISTS (SELECT 1 FROM replayed);
$$;
