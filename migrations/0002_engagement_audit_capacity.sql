CREATE TABLE engagements (
    id UUID PRIMARY KEY,
    contact_id UUID NOT NULL REFERENCES contacts (id) ON DELETE CASCADE,
    channel TEXT NOT NULL,
    direction TEXT NOT NULL,
    kind TEXT NOT NULL,
    subject TEXT,
    body TEXT,
    sequence_step SMALLINT,
    occurred_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX engagements_contact_idx ON engagements (contact_id, occurred_at DESC);

CREATE TABLE signals (
    id UUID PRIMARY KEY,
    company_domain TEXT NOT NULL,
    kind TEXT NOT NULL,
    strength TEXT NOT NULL,
    summary TEXT NOT NULL,
    source_url TEXT,
    detected_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX signals_domain_idx ON signals (company_domain, detected_at DESC);

CREATE TABLE sequence_states (
    contact_id UUID PRIMARY KEY REFERENCES contacts (id) ON DELETE CASCADE,
    cadence TEXT NOT NULL,
    current_step SMALLINT NOT NULL,
    max_steps SMALLINT NOT NULL,
    last_sent_at TIMESTAMPTZ,
    stopped BOOLEAN NOT NULL,
    stop_reason TEXT
);

CREATE TABLE audit_history (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    property TEXT NOT NULL,
    old_value JSONB,
    new_value JSONB NOT NULL,
    confidence REAL,
    updated_by TEXT NOT NULL,
    changed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX audit_entity_idx ON audit_history (entity_type, entity_id, changed_at DESC);

CREATE TABLE send_capacity (
    sender TEXT NOT NULL,
    day DATE NOT NULL,
    sent INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (sender, day)
);
