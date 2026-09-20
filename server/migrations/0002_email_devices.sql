ALTER TABLE users ADD COLUMN email TEXT UNIQUE;
ALTER TABLE users ADD COLUMN email_verified_at TIMESTAMPTZ;
CREATE TABLE email_challenges (
 email TEXT PRIMARY KEY, request_id UUID UNIQUE NOT NULL, code_hash BYTEA NOT NULL,
 requested_at TIMESTAMPTZ NOT NULL DEFAULT now(), expires_at TIMESTAMPTZ NOT NULL,
 attempts INT NOT NULL DEFAULT 0, ready BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX email_challenges_expiry_idx ON email_challenges(expires_at);
CREATE TABLE email_rate_limits (
 key TEXT PRIMARY KEY, hits INT NOT NULL, expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX email_rate_expiry_idx ON email_rate_limits(expires_at);
ALTER TABLE devices ADD COLUMN installation_hash TEXT UNIQUE;
ALTER TABLE devices ADD COLUMN leigod_account_key TEXT;
ALTER TABLE devices ADD COLUMN leigod_account_label TEXT;
ALTER TABLE devices ADD COLUMN account_updated_at TIMESTAMPTZ;
ALTER TABLE devices ALTER COLUMN game_running DROP NOT NULL;
