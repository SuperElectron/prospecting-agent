CREATE TABLE IF NOT EXISTS contacts_email_case_dedupe_backup (LIKE contacts INCLUDING DEFAULTS);

CREATE TEMP TABLE email_case_merge ON COMMIT DROP AS
WITH ranked AS (
    SELECT id,
           status,
           lower(btrim(email)) AS norm,
           row_number() OVER (
               PARTITION BY lower(btrim(email))
               ORDER BY (crm_id IS NOT NULL) DESC, created_at ASC, id ASC
           ) AS keep_rank
    FROM contacts
    WHERE email IS NOT NULL
)
SELECT loser.id AS loser_id, winner.id AS winner_id, loser.status AS loser_status
FROM ranked loser
JOIN ranked winner ON winner.norm = loser.norm AND winner.keep_rank = 1
WHERE loser.keep_rank > 1;

INSERT INTO contacts_email_case_dedupe_backup
SELECT c.* FROM contacts c JOIN email_case_merge m ON m.loser_id = c.id;

UPDATE engagements e
SET contact_id = m.winner_id
FROM email_case_merge m
WHERE e.contact_id = m.loser_id;

UPDATE agent_tasks t
SET contact_id = m.winner_id
FROM email_case_merge m
WHERE t.contact_id = m.loser_id;

UPDATE processed_inbound p
SET contact_id = m.winner_id
FROM email_case_merge m
WHERE p.contact_id = m.loser_id;

DELETE FROM sequence_states s
USING email_case_merge m
WHERE s.contact_id = m.loser_id
  AND EXISTS (SELECT 1 FROM sequence_states w WHERE w.contact_id = m.winner_id);

UPDATE sequence_states s
SET contact_id = m.winner_id
FROM email_case_merge m
WHERE s.contact_id = m.loser_id;

UPDATE contacts c
SET status = 'opted_out', updated_at = now()
FROM email_case_merge m
WHERE c.id = m.winner_id
  AND m.loser_status = 'opted_out'
  AND c.status <> 'opted_out';

UPDATE contacts c
SET status = 'disqualified', updated_at = now()
FROM email_case_merge m
WHERE c.id = m.winner_id
  AND m.loser_status = 'disqualified'
  AND c.status NOT IN ('opted_out', 'disqualified');

DELETE FROM contacts c USING email_case_merge m WHERE c.id = m.loser_id;

UPDATE contacts
SET email = lower(btrim(email))
WHERE email IS NOT NULL AND email <> lower(btrim(email));

DROP INDEX IF EXISTS contacts_email_unique;
CREATE UNIQUE INDEX contacts_email_unique ON contacts (lower(email)) WHERE email IS NOT NULL;
