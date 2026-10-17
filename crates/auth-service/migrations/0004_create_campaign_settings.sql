-- What whoever runs the services set for a campaign through the admin
-- API (read-only, limits of its own); carried in its tokens.
CREATE TABLE campaign_settings (
    campaign_id UUID PRIMARY KEY REFERENCES campaigns (id) ON DELETE CASCADE,
    settings    JSONB NOT NULL
);
