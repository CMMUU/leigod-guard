-- Credentials and decisions are account scoped; authorization remains device scoped.
CREATE TABLE remote_accounts (
 id UUID PRIMARY KEY,
 user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 provider_key TEXT UNIQUE NOT NULL,
 label TEXT NOT NULL,
 credential BYTEA,
 credential_version BIGINT NOT NULL DEFAULT 1,
 credential_state TEXT NOT NULL DEFAULT 'valid' CHECK (credential_state IN ('valid','reauthorize','deleted')),
 epoch BIGINT NOT NULL DEFAULT 1,
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX remote_accounts_owner_idx ON remote_accounts(user_id);
ALTER TABLE devices ADD COLUMN run_generation BIGINT NOT NULL DEFAULT 0;
ALTER TABLE devices ADD COLUMN remote_revision BIGINT NOT NULL DEFAULT 0;
CREATE TABLE remote_grants (
 device_id UUID PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
 account_id UUID NOT NULL REFERENCES remote_accounts(id),
 revision BIGINT NOT NULL DEFAULT 1,
 enabled BOOLEAN NOT NULL DEFAULT true,
 armed_at TIMESTAMPTZ,
 last_seen TIMESTAMPTZ,
 prepare_until TIMESTAMPTZ,
 run_generation BIGINT NOT NULL DEFAULT 0,
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX remote_grants_account_idx ON remote_grants(account_id) WHERE enabled;
CREATE TABLE remote_jobs (
 id UUID PRIMARY KEY,
 account_id UUID NOT NULL REFERENCES remote_accounts(id) ON DELETE CASCADE,
 epoch BIGINT NOT NULL,
 credential_version BIGINT NOT NULL,
 state TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued','running','confirmed','unconfirmed','cancelled','reauthorize')),
 attempts INTEGER NOT NULL DEFAULT 0,
 next_attempt TIMESTAMPTZ NOT NULL DEFAULT now(),
 expires_at TIMESTAMPTZ NOT NULL DEFAULT now()+interval '5 minutes',
 lease_id UUID,
 lease_until TIMESTAMPTZ,
 result TEXT NOT NULL DEFAULT 'waiting',
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 UNIQUE(account_id,epoch)
);
CREATE INDEX remote_jobs_queue_idx ON remote_jobs(next_attempt) WHERE state IN ('queued','running');
CREATE TABLE remote_service (
 singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
 warmup_until TIMESTAMPTZ NOT NULL DEFAULT now()+interval '120 seconds',
 blocked BOOLEAN NOT NULL DEFAULT false,
 reason TEXT NOT NULL DEFAULT 'startup',
 last_tick TIMESTAMPTZ NOT NULL DEFAULT now(),
 ingress_ok BOOLEAN NOT NULL DEFAULT false
);
INSERT INTO remote_service(singleton) VALUES(true);

-- All remote mutations serialize briefly here; never hold this lock over HTTP.
CREATE FUNCTION remote_cancel(a UUID) RETURNS VOID LANGUAGE plpgsql AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(73940128);
 UPDATE remote_accounts SET epoch=epoch+1,updated_at=now() WHERE id=a;
 UPDATE remote_jobs SET state='cancelled',result='authorization_or_heartbeat_changed',updated_at=now()
 WHERE account_id=a AND state IN ('queued','running');
 IF NOT EXISTS(SELECT 1 FROM remote_grants WHERE account_id=a AND enabled) THEN
  UPDATE remote_accounts SET credential=NULL,credential_state='deleted',credential_version=credential_version+1 WHERE id=a;
 END IF;
END $$;
CREATE FUNCTION remote_revoke_device(d UUID) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE a UUID; r BIGINT;
BEGIN
 PERFORM pg_advisory_xact_lock(73940128);
 UPDATE devices SET remote_revision=remote_revision+1 WHERE id=d RETURNING remote_revision INTO r;
 UPDATE remote_grants SET enabled=false,revision=r,armed_at=NULL,updated_at=now()
 WHERE device_id=d RETURNING account_id INTO a;
 IF a IS NOT NULL THEN PERFORM remote_cancel(a); END IF;
END $$;
CREATE FUNCTION remote_on_device_revoke() RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.revoked OR NEW.user_id<>OLD.user_id OR NEW.token_hash<>OLD.token_hash THEN
  PERFORM remote_revoke_device(NEW.id);
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER remote_device_revoke AFTER UPDATE OF revoked,user_id,token_hash ON devices
 FOR EACH ROW EXECUTE FUNCTION remote_on_device_revoke();
