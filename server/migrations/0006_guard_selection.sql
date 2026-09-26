-- NULL preserves the protocol for older clients. New clients opt in atomically.
ALTER TABLE devices ADD COLUMN guard_provider TEXT CHECK (guard_provider IN ('leigod','etalien'));
ALTER TABLE devices ADD COLUMN guard_revision BIGINT NOT NULL DEFAULT 0 CHECK (guard_revision >= 0);
