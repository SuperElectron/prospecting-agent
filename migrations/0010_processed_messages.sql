CREATE TABLE processed_inbound (
    gmail_message_id TEXT PRIMARY KEY,
    sender_email TEXT NOT NULL,
    contact_id UUID,
    processed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
