-- Every event that can enter RabbitMQ must retain a complete W3C carrier.
-- Repair the original research fixtures before making the contract strict;
-- new writes then fail at the durable boundary instead of publishing a
-- message that consumers cannot safely process.

SET TIME ZONE 'UTC';
SET search_path TO public;

UPDATE outbox_events
SET tracestate = 'playground=commerce'
WHERE tracestate IS NULL;

ALTER TABLE outbox_events
    ALTER COLUMN traceparent SET NOT NULL,
    ALTER COLUMN tracestate SET NOT NULL,
    ALTER COLUMN baggage SET NOT NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conrelid = 'public.outbox_events'::regclass
          AND conname = 'outbox_propagation_nonempty'
    ) THEN
        ALTER TABLE outbox_events
            ADD CONSTRAINT outbox_propagation_nonempty
            CHECK (
                length(btrim(traceparent)) > 0
                AND length(btrim(tracestate)) > 0
                AND length(btrim(baggage)) > 0
            );
    END IF;
END
$$;
