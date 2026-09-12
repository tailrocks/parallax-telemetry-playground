-- Fence fulfillment workers with durable, bounded claim leases.
-- Existing in-flight claims are retried after this migration instead of being
-- trusted without an owner token.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE fulfillment_processed_events
    ADD COLUMN IF NOT EXISTS lease_token TEXT,
    ADD COLUMN IF NOT EXISTS lease_until TIMESTAMPTZ;

UPDATE fulfillment_processed_events
   SET status = 'failed',
       last_error = 'claim lease migration reset',
       claimed_at = CURRENT_TIMESTAMP,
       completed_at = NULL,
       lease_token = NULL,
       lease_until = NULL
 WHERE status = 'processing';

CREATE INDEX IF NOT EXISTS idx_fulfillment_processed_active_lease
    ON fulfillment_processed_events (lease_until)
    WHERE status = 'processing';
