-- The key of the device that sent each command, so `undo` can tell whose
-- commands are whose. Commands from before this was recorded have none and
-- can't be undone.
ALTER TABLE combat_commands ADD COLUMN actor_key TEXT;
