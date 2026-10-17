-- Stat blocks, and the text of spells and conditions: a shared library
-- (campaign NULL, the SRD, imported by an admin) and each campaign's own.
-- The content is kept whole as JSON; what's queried on its own is a column.
CREATE TABLE monsters (
    id         UUID PRIMARY KEY,
    campaign   UUID,
    name       TEXT NOT NULL,
    source     TEXT NOT NULL,
    -- The rules it's written for: "2014" or "2024", or NULL for either.
    edition    TEXT,
    sheet      JSONB NOT NULL,
    version    BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX monsters_campaign_idx ON monsters (campaign, lower(name));

CREATE TABLE rules_text (
    id         UUID PRIMARY KEY,
    -- "spells" or "conditions".
    kind       TEXT NOT NULL,
    campaign   UUID,
    name       TEXT NOT NULL,
    source     TEXT NOT NULL,
    edition    TEXT,
    body       JSONB NOT NULL,
    version    BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX rules_text_kind_idx ON rules_text (kind, campaign, lower(name));
