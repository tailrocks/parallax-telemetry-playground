-- Make failed compensation visible and terminal after the bounded retry
-- budget. A terminal task is retained for operator reconciliation; it is not
-- silently recycled forever and does not block unrelated orders.

SET TIME ZONE 'UTC';
SET search_path TO public;

ALTER TABLE checkout_compensation_tasks
    DROP CONSTRAINT IF EXISTS checkout_compensation_tasks_status_check;

ALTER TABLE checkout_compensation_tasks
    ADD CONSTRAINT checkout_compensation_tasks_status_check
    CHECK (status IN ('queued', 'processing', 'completed', 'failed'));
