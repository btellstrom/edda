-- Planned encounters: reusable, per campaign. Loading one starts a new
-- combat from it (done by the client, which has the monsters' stat
-- blocks); the plan stays.
CREATE TABLE encounters (
    id         UUID PRIMARY KEY,
    campaign   TEXT NOT NULL,
    name       TEXT NOT NULL,
    plan       JSONB NOT NULL,
    version    BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX encounters_campaign_idx ON encounters (campaign, name);
