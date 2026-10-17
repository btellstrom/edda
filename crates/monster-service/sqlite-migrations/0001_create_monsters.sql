-- Stat blocks, as JSON in `sheet` like character sheets; the columns are
-- for finding them. `campaign` is NULL for the shared library (the SRD),
-- which every campaign of a matching `edition` sees (NULL suits any).
CREATE TABLE monsters (
    id         TEXT PRIMARY KEY NOT NULL,
    campaign   TEXT,
    name       TEXT NOT NULL,
    source     TEXT NOT NULL,
    edition    TEXT,
    sheet      TEXT NOT NULL,
    version    INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX monsters_campaign_idx ON monsters (campaign, name COLLATE NOCASE);
