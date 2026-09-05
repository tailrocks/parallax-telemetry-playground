-- Preserve the originating W3C carrier for durable compensation work.
-- The worker creates a fresh span per attempt and sanitizes this carrier
-- before invoking downstream services.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE checkout_compensation_tasks
    ADD COLUMN IF NOT EXISTS traceparent TEXT,
    ADD COLUMN IF NOT EXISTS tracestate TEXT,
    ADD COLUMN IF NOT EXISTS baggage TEXT;
