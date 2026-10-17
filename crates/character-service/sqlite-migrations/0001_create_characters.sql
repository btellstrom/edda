-- The full sheet lives in `sheet` as JSON so the schema can evolve without
-- migrations; frequently-queried fields are promoted to columns.
CREATE TABLE characters (
    id         TEXT PRIMARY KEY NOT NULL,
    name       TEXT NOT NULL,
    sheet      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX characters_name_idx ON characters (name);
