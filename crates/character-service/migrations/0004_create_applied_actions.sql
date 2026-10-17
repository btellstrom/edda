-- Actions sent with an Idempotency-Key, so a retried one isn't applied
-- twice (the combat service spends and gives back slots this way).
CREATE TABLE applied_actions (
    character  TEXT NOT NULL,
    key        TEXT NOT NULL,
    applied_at TEXT NOT NULL,
    PRIMARY KEY (character, key)
);
