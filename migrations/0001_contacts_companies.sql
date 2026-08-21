
CREATE TABLE companies (
    id UUID PRIMARY KEY,
    domain TEXT NOT NULL UNIQUE,
    name TEXT,
    industry TEXT,
    employee_count INTEGER CHECK (employee_count >= 0),
    location TEXT,
    linkedin_url TEXT,
    crm_id TEXT,
    hiring_velocity TEXT,
    icp_fit_score SMALLINT CHECK (icp_fit_score BETWEEN 0 AND 255),
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
    score SMALLINT CHECK (score BETWEEN 0 AND 255),
    status TEXT NOT NULL,
    assigned_sender TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX contacts_email_unique ON contacts (email) WHERE email IS NOT NULL;
CREATE INDEX contacts_company_domain_idx ON contacts (company_domain);
CREATE INDEX contacts_status_idx ON contacts (status, updated_at DESC);
