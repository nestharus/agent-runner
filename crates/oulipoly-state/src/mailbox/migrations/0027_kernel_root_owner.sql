-- The broker root incarnation is part of the durable completion owner row.
-- Pre-existing ordinary owners remain NULL; pinned owners publish a canonical
-- root UUID in the same transaction as guardian and supervisor identity.
ALTER TABLE completion_continuation_owner ADD COLUMN kernel_root_id TEXT;
CREATE INDEX completion_continuation_owner_kernel_root
ON completion_continuation_owner(kernel_root_id)
WHERE kernel_root_id IS NOT NULL;
