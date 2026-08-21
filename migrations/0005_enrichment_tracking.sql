ALTER TABLE companies ADD COLUMN enrichment_attempted_at TIMESTAMPTZ;

DELETE FROM contacts a USING contacts b
WHERE a.id > b.id AND a.crm_id IS NOT NULL AND a.crm_id = b.crm_id;

CREATE UNIQUE INDEX contacts_crm_id_key ON contacts (crm_id) WHERE crm_id IS NOT NULL;
