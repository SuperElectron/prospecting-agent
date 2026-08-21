UPDATE signals SET company_domain = lower(company_domain);

DELETE FROM engagements a USING engagements b
WHERE a.direction = 'outbound' AND b.direction = 'outbound'
  AND a.kind = 'sent' AND b.kind = 'sent'
  AND a.contact_id = b.contact_id
  AND a.channel = b.channel
  AND a.sequence_step IS NOT NULL
  AND a.sequence_step = b.sequence_step
  AND (a.occurred_at, a.id) > (b.occurred_at, b.id);

CREATE UNIQUE INDEX engagements_send_dedupe_idx ON engagements (
    contact_id, channel, sequence_step
) WHERE direction = 'outbound' AND kind = 'sent' AND sequence_step IS NOT NULL;

DELETE FROM signals s USING signals keep
WHERE s.company_domain = keep.company_domain
  AND s.kind = keep.kind
  AND md5(s.summary) = md5(keep.summary)
  AND (s.detected_at, s.id) < (keep.detected_at, keep.id);

CREATE UNIQUE INDEX signals_dedupe_idx ON signals (company_domain, kind, md5(summary));
