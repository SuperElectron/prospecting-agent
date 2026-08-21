ALTER TABLE sequence_states ADD COLUMN stopped_at TIMESTAMPTZ;

UPDATE sequence_states SET stopped_at = now() WHERE stopped = true;
