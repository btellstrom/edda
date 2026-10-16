-- A combat's state is never stored directly: it is rebuilt by replaying
-- combat_events in seq order. status/round are a denormalised summary kept
-- up to date for listing.
CREATE TABLE combats (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL,
    status     TEXT NOT NULL,
    round      INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

-- The complete, append-only log of everything that happened in a combat.
CREATE TABLE combat_events (
    combat_id  TEXT NOT NULL REFERENCES combats (id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    command_id TEXT,
    actor      TEXT,
    at         TEXT NOT NULL,
    event      TEXT NOT NULL,
    PRIMARY KEY (combat_id, seq)
);

CREATE INDEX combat_events_command_idx ON combat_events (combat_id, command_id);
