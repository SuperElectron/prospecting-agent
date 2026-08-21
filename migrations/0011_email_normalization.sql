CREATE TABLE IF NOT EXISTS contacts_email_case_dedupe_backup (LIKE contacts INCLUDING DEFAULTS);

WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY lower(btrim(email))
               ORDER BY (crm_id IS NOT NULL) DESC, created_at ASC, id ASC
           ) AS keep_rank
    FROM contacts
    WHERE email IS NOT NULL
),
losers AS (
    SELECT id FROM ranked WHERE keep_rank > 1
)
INSERT INTO contacts_email_case_dedupe_backup
SELECT c.* FROM contacts c JOIN losers l ON l.id = c.id;

DELETE FROM contacts
WHERE email IS NOT NULL
  AND id IN (SELECT id FROM contacts_email_case_dedupe_backup);

UPDATE contacts
SET email = lower(btrim(email))
WHERE email IS NOT NULL AND email <> lower(btrim(email));
