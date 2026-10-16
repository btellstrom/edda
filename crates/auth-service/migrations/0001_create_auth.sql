CREATE TABLE campaigns (
    id         UUID PRIMARY KEY,
    name       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);

-- One-time codes that let a device register itself as a campaign's DM:
-- printed when a campaign is created, or by an admin to recover a campaign
-- whose DM devices are lost. Only a SHA-256 of the code is stored.
CREATE TABLE setup_codes (
    code_hash   BYTEA PRIMARY KEY,
    campaign_id UUID NOT NULL REFERENCES campaigns (id) ON DELETE CASCADE,
    expires_at  TIMESTAMPTZ NOT NULL
);

-- Devices admitted to a campaign, each with the certificate that vouches
-- for it. Revoked devices are kept for the record.
CREATE TABLE devices (
    campaign_id UUID NOT NULL REFERENCES campaigns (id) ON DELETE CASCADE,
    public_key  BYTEA NOT NULL,
    role        TEXT NOT NULL,
    name        TEXT NOT NULL,
    cert        JSONB NOT NULL,
    added_at    TIMESTAMPTZ NOT NULL,
    revoked_at  TIMESTAMPTZ,
    PRIMARY KEY (campaign_id, public_key)
);

-- Other services, which may log in to any campaign.
CREATE TABLE services (
    public_key BYTEA PRIMARY KEY,
    name       TEXT NOT NULL,
    added_at   TIMESTAMPTZ NOT NULL
);

-- Outstanding login challenges. Each is deleted as it is answered, so it
-- can be used once, whichever instance receives the answer.
CREATE TABLE challenges (
    id          UUID PRIMARY KEY,
    campaign_id UUID NOT NULL,
    public_key  BYTEA NOT NULL,
    nonce       BYTEA NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL
);

-- The key tokens are signed with; one row, shared by all instances.
CREATE TABLE token_key (
    id   BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id),
    seed BYTEA NOT NULL
);
