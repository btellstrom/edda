-- The player devices that play a character, by key, as a JSON array. They
-- may change it; DM devices may change any. Characters from before owners
-- existed have none until the DM assigns them (campaign-edit <CHARACTER>
-- owners add <DEVICE>).
ALTER TABLE characters ADD COLUMN owners TEXT NOT NULL DEFAULT '[]';
