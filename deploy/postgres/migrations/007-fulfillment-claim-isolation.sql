-- Make fulfillment claim identity tenant-scoped and bound durable retries.
-- Existing rows are preserved; exhausted rows become terminal dead letters.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE fulfillment_processed_events
    ADD COLUMN IF NOT EXISTS dead_lettered_at TIMESTAMPTZ;

ALTER TABLE fulfillment_processed_events
    DROP CONSTRAINT IF EXISTS fulfillment_processed_events_status_check;

ALTER TABLE fulfillment_processed_events
    ADD CONSTRAINT fulfillment_processed_events_status_check
    CHECK (status IN ('processing', 'failed', 'completed', 'dead_lettered'));

-- Old bootstraps allowed zero attempts. Normalize those rows before adding the
-- stricter invariant. Rows that already exhausted the limit cannot be safely
-- retried without a new operator decision, so retain them as dead letters.
UPDATE fulfillment_processed_events
   SET attempts = 1
 WHERE attempts < 1;

UPDATE fulfillment_processed_events
   SET attempts = LEAST(attempts, 5),
       status = 'dead_lettered',
       dead_lettered_at = COALESCE(dead_lettered_at, CURRENT_TIMESTAMP),
       completed_at = NULL,
       lease_token = NULL,
       lease_until = NULL,
       last_error = COALESCE(last_error, 'fulfillment retry limit exhausted')
 WHERE status IN ('processing', 'failed')
   AND attempts >= 5;

UPDATE fulfillment_processed_events
   SET attempts = 5
 WHERE attempts > 5;

ALTER TABLE fulfillment_processed_events
    DROP CONSTRAINT IF EXISTS fulfillment_processed_events_attempts_check;

ALTER TABLE fulfillment_processed_events
    DROP CONSTRAINT IF EXISTS fulfillment_processed_events_attempts_positive_check;

ALTER TABLE fulfillment_processed_events
    ADD CONSTRAINT fulfillment_processed_events_attempts_positive_check
    CHECK (attempts > 0);

ALTER TABLE fulfillment_processed_events
    DROP CONSTRAINT IF EXISTS fulfillment_processed_events_attempts_max_check;

ALTER TABLE fulfillment_processed_events
    ADD CONSTRAINT fulfillment_processed_events_attempts_max_check
    CHECK (attempts <= 5);

-- The old primary key allowed one tenant's event key to fence another tenant.
-- Replace it with the complete business identity. The guarded block keeps a
-- second migration run harmless.
DO $$
DECLARE
    current_pk TEXT;
BEGIN
    IF NOT EXISTS (
        SELECT 1
          FROM pg_constraint c
          JOIN pg_class t ON t.oid = c.conrelid
          JOIN pg_namespace n ON n.oid = t.relnamespace
         WHERE n.nspname = 'public'
           AND t.relname = 'fulfillment_processed_events'
           AND c.contype = 'p'
           AND pg_get_constraintdef(c.oid) =
               'PRIMARY KEY (tenant_id, consumer_name, event_key)'
    ) THEN
        SELECT c.conname
          INTO current_pk
          FROM pg_constraint c
          JOIN pg_class t ON t.oid = c.conrelid
          JOIN pg_namespace n ON n.oid = t.relnamespace
         WHERE n.nspname = 'public'
           AND t.relname = 'fulfillment_processed_events'
           AND c.contype = 'p';

        IF current_pk IS NOT NULL THEN
            EXECUTE format(
                'ALTER TABLE public.fulfillment_processed_events DROP CONSTRAINT %I',
                current_pk
            );
        END IF;

        ALTER TABLE public.fulfillment_processed_events
            ADD CONSTRAINT fulfillment_processed_events_pkey
            PRIMARY KEY (tenant_id, consumer_name, event_key);
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS idx_fulfillment_processed_retry
    ON fulfillment_processed_events (status, claimed_at)
    WHERE status IN ('processing', 'failed');
