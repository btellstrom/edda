-- A combat's state is never stored directly: it is rebuilt by replaying
-- combat_events in seq order. The other columns of combats are a summary
-- for listing, plus `seq`, the position of the latest event. Every command
-- locks its combat's row (SELECT ... FOR UPDATE), which is what serialises
-- commands across any number of service instances.
CREATE TABLE combats (
    id         UUID PRIMARY KEY,
    campaign   TEXT NOT NULL,
    name       TEXT NOT NULL,
    status     TEXT NOT NULL,
    round      INTEGER NOT NULL,
    seq        BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX combats_campaign_idx ON combats (campaign, updated_at DESC);

-- The complete, append-only log of everything that happened in a combat.
CREATE TABLE combat_events (
    combat_id  UUID NOT NULL REFERENCES combats (id) ON DELETE CASCADE,
    seq        BIGINT NOT NULL,
    command_id UUID,
    actor      TEXT,
    at         TIMESTAMPTZ NOT NULL,
    event      JSONB NOT NULL,
    PRIMARY KEY (combat_id, seq)
);

CREATE INDEX combat_events_command_idx ON combat_events (combat_id, command_id);

-- Every command id ever applied, so a resent command is recognised by
-- whichever instance receives it.
CREATE TABLE combat_commands (
    combat_id  UUID NOT NULL REFERENCES combats (id) ON DELETE CASCADE,
    command_id UUID NOT NULL,
    PRIMARY KEY (combat_id, command_id)
);

-- How far each combat's hit points have been written back to the
-- character service. A separate table from combats so that syncing (which
-- waits on HTTP) never holds the lock commands need.
CREATE TABLE combat_hp_sync (
    combat_id  UUID PRIMARY KEY REFERENCES combats (id) ON DELETE CASCADE,
    synced_seq BIGINT NOT NULL
);
