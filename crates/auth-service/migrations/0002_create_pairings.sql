-- Pairings in progress: a DM device ("host") pairing a new device
-- ("joiner") with a short code. The server only relays their messages; the
-- code's words never reach it. Each side sends at most two messages.
CREATE TABLE pairings (
    slot        INTEGER PRIMARY KEY,
    campaign_id UUID NOT NULL REFERENCES campaigns (id) ON DELETE CASCADE,
    host_key    BYTEA NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL
);

CREATE TABLE pairing_messages (
    slot INTEGER NOT NULL REFERENCES pairings (slot) ON DELETE CASCADE,
    side TEXT NOT NULL,
    n    SMALLINT NOT NULL,
    data BYTEA NOT NULL,
    PRIMARY KEY (slot, side, n)
);
