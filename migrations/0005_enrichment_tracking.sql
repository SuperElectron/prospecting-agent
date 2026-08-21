ALTER TABLE companies ADD COLUMN IF NOT EXISTS enrichment_attempted_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS contacts_crm_dedupe_backup (LIKE contacts INCLUDING DEFAULTS);

WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY crm_id
               ORDER BY (email IS NOT NULL) DESC, created_at ASC, id ASC
           ) AS keep_rank
    FROM contacts
    WHERE crm_id IS NOT NULL
),
losers AS (
    SELECT id FROM ranked WHERE keep_rank > 1
)
INSERT INTO contacts_crm_dedupe_backup
SELECT c.* FROM contacts c JOIN losers l ON l.id = c.id;

DELETE FROM contacts
WHERE crm_id IS NOT NULL
  AND id IN (SELECT id FROM contacts_crm_dedupe_backup);

CREATE UNIQUE INDEX IF NOT EXISTS contacts_crm_id_key ON contacts (crm_id) WHERE crm_id IS NOT NULL;
