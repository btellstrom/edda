-- Spells' and conditions' descriptions (`kind` says which), as JSON in
-- `body`, shared and per campaign exactly like monsters.
CREATE TABLE rules_text (
    id         TEXT PRIMARY KEY NOT NULL,
    kind       TEXT NOT NULL,
    campaign   TEXT,
    name       TEXT NOT NULL,
    source     TEXT NOT NULL,
    edition    TEXT,
    body       TEXT NOT NULL,
    version    INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX rules_text_kind_idx ON rules_text (kind, campaign, name COLLATE NOCASE);
