-- Characters, each in one campaign. The sheet is kept whole as JSON; what's
-- queried on its own is a column.
CREATE TABLE characters (
    id         UUID PRIMARY KEY,
    campaign   UUID NOT NULL,
    -- The player devices that play it, by key; DM devices may change any.
    owners     TEXT[] NOT NULL DEFAULT '{}',
    name       TEXT NOT NULL,
    sheet      JSONB NOT NULL,
    -- Incremented on every change; the ETag.
    version    BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX characters_campaign_idx ON characters (campaign, lower(name));

-- Actions applied with an idempotency key, so a retry applies nothing.
CREATE TABLE applied_actions (
    character  UUID NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    applied_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (character, key)
);
