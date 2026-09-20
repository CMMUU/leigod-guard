CREATE TABLE users (
 id UUID PRIMARY KEY, username TEXT UNIQUE NOT NULL,
 display_name TEXT NOT NULL, password_hash TEXT NOT NULL,
 role TEXT NOT NULL CHECK (role IN ('admin','user')) DEFAULT 'user',
 disabled BOOLEAN NOT NULL DEFAULT FALSE, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE sessions (
 token_hash TEXT PRIMARY KEY, user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 csrf TEXT NOT NULL, expires_at TIMESTAMPTZ NOT NULL,
 last_seen TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sessions_user_idx ON sessions(user_id);
CREATE INDEX sessions_expiry_idx ON sessions(expires_at);
CREATE TABLE pairing_codes (
 code_hash TEXT PRIMARY KEY, user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX pairing_user_idx ON pairing_codes(user_id);
CREATE TABLE devices (
 id UUID PRIMARY KEY, user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 name TEXT NOT NULL, token_hash TEXT UNIQUE NOT NULL, version TEXT NOT NULL DEFAULT '',
 last_seen TIMESTAMPTZ, sequence BIGINT NOT NULL DEFAULT -1,
 game_running BOOLEAN NOT NULL DEFAULT FALSE, prepare_until TIMESTAMPTZ,
 revoked BOOLEAN NOT NULL DEFAULT FALSE, observed_offline BOOLEAN NOT NULL DEFAULT FALSE,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX devices_user_idx ON devices(user_id);
CREATE INDEX devices_active_seen_idx ON devices(last_seen) WHERE NOT revoked;
CREATE TABLE events (
 id BIGSERIAL PRIMARY KEY, user_id UUID REFERENCES users(id) ON DELETE CASCADE,
 device_id UUID REFERENCES devices(id) ON DELETE SET NULL,
 kind TEXT NOT NULL, detail TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX events_user_time_idx ON events(user_id,created_at DESC);
CREATE INDEX events_time_idx ON events(created_at DESC);
CREATE TABLE metrics (
 bucket TIMESTAMPTZ PRIMARY KEY, online_devices BIGINT NOT NULL,
 online_users BIGINT NOT NULL, web_users BIGINT NOT NULL
);
CREATE TABLE service_state (key TEXT PRIMARY KEY, updated_at TIMESTAMPTZ NOT NULL);
