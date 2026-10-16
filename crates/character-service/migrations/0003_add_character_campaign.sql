-- The campaign (from the auth service) a character belongs to. Characters
-- created before campaigns existed have none and are invisible until one is
-- assigned: UPDATE characters SET campaign = '<campaign id>' WHERE campaign IS NULL;
ALTER TABLE characters ADD COLUMN campaign TEXT;

CREATE INDEX characters_campaign_idx ON characters (campaign, name);
