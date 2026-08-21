CREATE TABLE account_strategies (
    domain TEXT PRIMARY KEY,
    stage TEXT NOT NULL CHECK (
        stage IN ('prospecting', 'engaged', 'opportunity', 'multi_threaded', 'customer', 'dormant')
    ),
    health TEXT NOT NULL CHECK (health IN ('healthy', 'watch', 'blocked')),
    coordination_flags JSONB NOT NULL DEFAULT '[]'::jsonb,
    summary TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
