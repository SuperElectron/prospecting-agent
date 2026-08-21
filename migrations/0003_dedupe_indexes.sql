DELETE FROM engagements a USING engagements b
WHERE a.id > b.id
  AND a.direction = 'outbound' AND b.direction = 'outbound'
  AND a.kind = 'sent' AND b.kind = 'sent'
  AND a.contact_id = b.contact_id
  AND a.channel = b.channel
  AND COALESCE(a.sequence_step, -1) = COALESCE(b.sequence_step, -1);

CREATE UNIQUE INDEX engagements_send_dedupe_idx ON engagements (
    contact_id, channel, COALESCE(sequence_step, -1)
) WHERE direction = 'outbound' AND kind = 'sent';

DELETE FROM signals a USING signals b
WHERE a.id > b.id
  AND a.company_domain = b.company_domain
  AND a.kind = b.kind
  AND a.summary = b.summary;

CREATE UNIQUE INDEX signals_dedupe_idx ON signals (company_domain, kind, summary);
