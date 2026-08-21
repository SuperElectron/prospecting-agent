CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE companies (
    id UUID PRIMARY KEY,
    domain TEXT NOT NULL UNIQUE,
    name TEXT,
    industry TEXT,
    employee_count INTEGER,
    location TEXT,
    linkedin_url TEXT,
    crm_id TEXT,
    hiring_velocity TEXT,
    icp_fit_score SMALLINT,
    summary TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE contacts (
    id UUID PRIMARY KEY,
    email TEXT,
    first_name TEXT,
    last_name TEXT,
    title TEXT,
    seniority TEXT,
    linkedin_url TEXT,
    company_domain TEXT,
    crm_id TEXT,
    source TEXT NOT NULL,
    score SMALLINT,
    status TEXT NOT NULL,
    assigned_sender TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX contacts_email_unique ON contacts (email) WHERE email IS NOT NULL;
CREATE INDEX contacts_company_domain_idx ON contacts (company_domain);
CREATE INDEX contacts_status_idx ON contacts (status);
