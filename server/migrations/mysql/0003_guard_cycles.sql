-- Additive v2 protocol. Existing grants remain on v1 until explicit consent.
CREATE TABLE guard_accounts (
 account_id BINARY(16) PRIMARY KEY, cycle_id BINARY(16) NULL,
 startup_enabled BOOLEAN NOT NULL DEFAULT TRUE,
 startup_seconds INT NOT NULL DEFAULT 180, exit_seconds INT NOT NULL DEFAULT 90,
 policy_revision BIGINT NOT NULL DEFAULT 1,
 FOREIGN KEY(account_id) REFERENCES remote_accounts(id) ON DELETE CASCADE
) ENGINE=InnoDB;
CREATE TABLE guard_cycles (
 id BINARY(16) PRIMARY KEY, account_id BINARY(16) NOT NULL,
 state VARCHAR(16) NOT NULL DEFAULT 'running', revision BIGINT NOT NULL DEFAULT 1,
 started_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), completed_at TIMESTAMP(6) NULL,
 FOREIGN KEY(account_id) REFERENCES remote_accounts(id) ON DELETE CASCADE,
 INDEX guard_cycles_account(account_id,started_at)
) ENGINE=InnoDB;
CREATE TABLE guard_observers (
 device_id BINARY(16) NOT NULL, provider VARCHAR(8) NOT NULL,
 account_id BINARY(16) NOT NULL, grant_revision BIGINT NOT NULL,
 run_generation BIGINT NOT NULL, sequence BIGINT NOT NULL DEFAULT -1,
 enabled BOOLEAN NOT NULL DEFAULT TRUE, cycle_id BINARY(16) NULL,
 game_generation BIGINT UNSIGNED NOT NULL DEFAULT 0,
 phase VARCHAR(12) NOT NULL DEFAULT 'unknown', activity BOOLEAN NOT NULL DEFAULT FALSE,
 observed_at TIMESTAMP(6) NULL, executor_at TIMESTAMP(6) NULL,
 seen_at TIMESTAMP(6) NULL, idle_until TIMESTAMP(6) NULL, prepare_until TIMESTAMP(6) NULL,
 billing_at TIMESTAMP(6) NULL,
 PRIMARY KEY(device_id,provider),
 FOREIGN KEY(device_id) REFERENCES devices(id) ON DELETE CASCADE,
 FOREIGN KEY(account_id) REFERENCES remote_accounts(id) ON DELETE CASCADE,
 INDEX guard_observers_account(account_id,enabled)
) ENGINE=InnoDB;
CREATE TABLE guard_actions (
 id BINARY(16) PRIMARY KEY, account_id BINARY(16) NOT NULL, cycle_id BINARY(16) NOT NULL,
 device_id BINARY(16) NULL, run_generation BIGINT NOT NULL,
 credential_version BIGINT NOT NULL, cycle_revision BIGINT NOT NULL,
 request_id VARCHAR(64) NOT NULL, state VARCHAR(16) NOT NULL DEFAULT 'leased',
 is_manual BOOLEAN NOT NULL DEFAULT FALSE, lease_until TIMESTAMP(6) NOT NULL,
 result VARCHAR(64) NOT NULL DEFAULT 'reserved',
 created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
 FOREIGN KEY(account_id) REFERENCES remote_accounts(id) ON DELETE CASCADE,
 FOREIGN KEY(cycle_id) REFERENCES guard_cycles(id) ON DELETE CASCADE,
 UNIQUE(account_id,request_id), INDEX guard_actions_pending(state,lease_until)
) ENGINE=InnoDB;
