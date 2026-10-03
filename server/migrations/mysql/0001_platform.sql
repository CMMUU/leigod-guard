-- Independent MySQL history. PostgreSQL migrations remain immutable for rollback.
-- UTC session, explicit nullability, binary UUIDs and case-sensitive identity keys.
CREATE TABLE guard_lock (id TINYINT PRIMARY KEY) ENGINE=InnoDB;
INSERT INTO guard_lock VALUES(1);
CREATE TABLE users (
 id BINARY(16) PRIMARY KEY, username VARCHAR(254) COLLATE utf8mb4_0900_as_cs NOT NULL UNIQUE,
 display_name VARCHAR(100) NOT NULL, password_hash VARCHAR(512) NOT NULL,
 role VARCHAR(8) NOT NULL DEFAULT 'user', disabled BOOLEAN NOT NULL DEFAULT FALSE,
 created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 email VARCHAR(254) COLLATE utf8mb4_0900_as_cs NULL UNIQUE, email_verified_at TIMESTAMP(6) NULL,
 CHECK(role IN ('admin','user'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE TABLE sessions (
 token_hash CHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs PRIMARY KEY,
 user_id BINARY(16) NOT NULL, csrf CHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs NOT NULL,
 expires_at TIMESTAMP(6) NOT NULL, last_seen TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
 INDEX sessions_user_idx(user_id), INDEX sessions_expiry_idx(expires_at)
) ENGINE=InnoDB;
CREATE TABLE pairing_codes (
 code_hash CHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs PRIMARY KEY,
 user_id BINARY(16) NOT NULL, expires_at TIMESTAMP(6) NOT NULL,
 FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE, INDEX pairing_user_idx(user_id)
) ENGINE=InnoDB;
CREATE TABLE devices (
 id BINARY(16) PRIMARY KEY, user_id BINARY(16) NOT NULL,
 name VARCHAR(100) NOT NULL, token_hash CHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs NOT NULL UNIQUE,
 version VARCHAR(32) NOT NULL DEFAULT '', last_seen TIMESTAMP(6) NULL,
 sequence BIGINT NOT NULL DEFAULT -1, game_running BOOLEAN NULL DEFAULT FALSE,
 prepare_until TIMESTAMP(6) NULL, revoked BOOLEAN NOT NULL DEFAULT FALSE,
 observed_offline BOOLEAN NOT NULL DEFAULT FALSE,
 created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 installation_hash CHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs NULL UNIQUE,
 leigod_account_key CHAR(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs NULL,
 leigod_account_label VARCHAR(100) NULL, account_updated_at TIMESTAMP(6) NULL,
 run_generation BIGINT NOT NULL DEFAULT 0, remote_revision BIGINT NOT NULL DEFAULT 0,
 etalien_revision BIGINT NOT NULL DEFAULT 0, guard_provider VARCHAR(8) NULL,
 guard_revision BIGINT NOT NULL DEFAULT 0,
 FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
 CHECK(guard_provider IN ('leigod','etalien')), CHECK(guard_revision>=0),
 INDEX devices_user_idx(user_id), INDEX devices_active_seen_idx(revoked,last_seen)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE TABLE events (
 id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, user_id BINARY(16) NULL,
 device_id BINARY(16) NULL, kind VARCHAR(64) NOT NULL, detail TEXT NOT NULL,
 created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
 FOREIGN KEY(device_id) REFERENCES devices(id) ON DELETE SET NULL,
 INDEX events_user_time_idx(user_id,created_at DESC), INDEX events_time_idx(created_at DESC)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE TABLE metrics (
 bucket TIMESTAMP(6) PRIMARY KEY, online_devices BIGINT NOT NULL,
 online_users BIGINT NOT NULL, web_users BIGINT NOT NULL
) ENGINE=InnoDB;
CREATE TABLE service_state (`key` VARCHAR(64) PRIMARY KEY, updated_at TIMESTAMP(6) NOT NULL) ENGINE=InnoDB;
CREATE TABLE email_challenges (
 email VARCHAR(254) COLLATE utf8mb4_0900_as_cs PRIMARY KEY, request_id BINARY(16) NOT NULL UNIQUE,
 code_hash VARBINARY(32) NOT NULL, requested_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 expires_at TIMESTAMP(6) NOT NULL, attempts INT NOT NULL DEFAULT 0, ready BOOLEAN NOT NULL DEFAULT FALSE,
 INDEX email_challenges_expiry_idx(expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE TABLE email_rate_limits (
 `key` VARCHAR(100) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs PRIMARY KEY,
 hits INT NOT NULL, expires_at TIMESTAMP(6) NOT NULL, INDEX email_rate_expiry_idx(expires_at)
) ENGINE=InnoDB;
CREATE TABLE remote_accounts (
 id BINARY(16) PRIMARY KEY, user_id BINARY(16) NOT NULL,
 provider_key VARCHAR(128) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_as_cs NOT NULL UNIQUE,
 label VARCHAR(100) NOT NULL, credential BLOB NULL,
 credential_version BIGINT NOT NULL DEFAULT 1, credential_state VARCHAR(16) NOT NULL DEFAULT 'valid',
 epoch BIGINT NOT NULL DEFAULT 1, updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 provider VARCHAR(8) NOT NULL DEFAULT 'leigod',
 CHECK(credential_state IN ('valid','reauthorize','deleted')), CHECK(provider IN ('leigod','etalien')),
 FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
 UNIQUE(id,provider), INDEX remote_accounts_owner_idx(user_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE TABLE remote_grants (
 device_id BINARY(16) NOT NULL, provider VARCHAR(8) NOT NULL DEFAULT 'leigod',
 account_id BINARY(16) NOT NULL, revision BIGINT NOT NULL DEFAULT 1,
 enabled BOOLEAN NOT NULL DEFAULT TRUE, armed_at TIMESTAMP(6) NULL, last_seen TIMESTAMP(6) NULL,
 prepare_until TIMESTAMP(6) NULL, run_generation BIGINT NOT NULL DEFAULT 0,
 updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 PRIMARY KEY(device_id,provider), CHECK(provider IN ('leigod','etalien')),
 FOREIGN KEY(device_id) REFERENCES devices(id) ON DELETE CASCADE,
 FOREIGN KEY(account_id,provider) REFERENCES remote_accounts(id,provider),
 INDEX remote_grants_account_idx(account_id,enabled)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE TABLE remote_jobs (
 id BINARY(16) PRIMARY KEY, account_id BINARY(16) NOT NULL,
 epoch BIGINT NOT NULL, credential_version BIGINT NOT NULL,
 state VARCHAR(16) NOT NULL DEFAULT 'queued', attempts INT NOT NULL DEFAULT 0,
 next_attempt TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 expires_at TIMESTAMP(6) NOT NULL DEFAULT (CURRENT_TIMESTAMP(6) + INTERVAL 5 MINUTE),
 lease_id BINARY(16) NULL, lease_until TIMESTAMP(6) NULL, result VARCHAR(100) NOT NULL DEFAULT 'waiting',
 created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), trigger_kind VARCHAR(8) NOT NULL DEFAULT 'offline',
 FOREIGN KEY(account_id) REFERENCES remote_accounts(id) ON DELETE CASCADE, UNIQUE(account_id,epoch),
 CHECK(state IN ('queued','running','confirmed','unconfirmed','cancelled','reauthorize')),
 CHECK(trigger_kind IN ('offline','cafe')), INDEX remote_jobs_queue_idx(state,next_attempt)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
CREATE TABLE remote_service (
 singleton BOOLEAN PRIMARY KEY DEFAULT TRUE,
 warmup_until TIMESTAMP(6) NOT NULL DEFAULT (CURRENT_TIMESTAMP(6) + INTERVAL 120 SECOND),
 blocked BOOLEAN NOT NULL DEFAULT FALSE, reason VARCHAR(64) NOT NULL DEFAULT 'startup',
 last_tick TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), ingress_ok BOOLEAN NOT NULL DEFAULT FALSE,
 CHECK(singleton=TRUE)
) ENGINE=InnoDB;
INSERT INTO remote_service(singleton) VALUES(TRUE);
CREATE TABLE cafe_policies (
 account_id BINARY(16) PRIMARY KEY, enabled BOOLEAN NOT NULL DEFAULT FALSE,
 max_hours INT NOT NULL DEFAULT 24, revision BIGINT NOT NULL DEFAULT 1,
 started_at TIMESTAMP(6) NULL, observed_at TIMESTAMP(6) NULL,
 observed_state VARCHAR(8) NOT NULL DEFAULT 'unknown',
 next_poll TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), poll_lease BINARY(16) NULL,
 poll_until TIMESTAMP(6) NULL, failures INT NOT NULL DEFAULT 0,
 updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 CHECK(max_hours BETWEEN 1 AND 168), CHECK(observed_state IN ('unknown','running','paused')),
 FOREIGN KEY(account_id) REFERENCES remote_accounts(id) ON DELETE CASCADE,
 INDEX cafe_poll_idx(enabled,next_poll)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_as_cs;
