-- Incremented on every write; exposed as the ETag for optimistic concurrency.
ALTER TABLE characters ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
