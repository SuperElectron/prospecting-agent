CREATE TABLE imported_notes (
    content_hash TEXT PRIMARY KEY,
    imported_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
