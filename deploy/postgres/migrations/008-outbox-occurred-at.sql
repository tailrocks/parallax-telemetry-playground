-- Preserve the business event timestamp across outbox retries and replay.
-- The publisher must never manufacture a new event time after a claim fails.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE outbox_events
    ADD COLUMN IF NOT EXISTS occurred_at TIMESTAMPTZ;

UPDATE outbox_events
SET occurred_at = created_at
WHERE occurred_at IS NULL;

ALTER TABLE outbox_events
    ALTER COLUMN occurred_at SET DEFAULT CURRENT_TIMESTAMP,
    ALTER COLUMN occurred_at SET NOT NULL;

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
