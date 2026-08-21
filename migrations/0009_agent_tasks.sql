CREATE TABLE agent_tasks (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL,
    contact_id UUID REFERENCES contacts(id) ON DELETE CASCADE,
    company_domain TEXT,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL,
    attempts SMALLINT NOT NULL DEFAULT 0,
    due_at TIMESTAMPTZ,
    claimed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX agent_tasks_claimable ON agent_tasks (status, due_at, created_at);
