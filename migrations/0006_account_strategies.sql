CREATE TABLE account_strategies (
    domain TEXT PRIMARY KEY,
    stage TEXT NOT NULL,
    health TEXT NOT NULL,
    coordination_flags JSONB NOT NULL DEFAULT '[]'::jsonb,
    summary TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
