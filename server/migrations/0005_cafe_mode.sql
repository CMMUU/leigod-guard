-- Explicit account-level cloud consent, independent of any installed device.
CREATE TABLE cafe_policies (
 account_id UUID PRIMARY KEY REFERENCES remote_accounts(id) ON DELETE CASCADE,
 enabled BOOLEAN NOT NULL DEFAULT false,
 max_hours INTEGER NOT NULL DEFAULT 24 CHECK(max_hours BETWEEN 1 AND 168),
 revision BIGINT NOT NULL DEFAULT 1,
 started_at TIMESTAMPTZ,
 observed_at TIMESTAMPTZ,
 observed_state TEXT NOT NULL DEFAULT 'unknown' CHECK(observed_state IN ('unknown','running','paused')),
 next_poll TIMESTAMPTZ NOT NULL DEFAULT now(),
 poll_lease UUID,
 poll_until TIMESTAMPTZ,
 failures INTEGER NOT NULL DEFAULT 0,
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX cafe_poll_idx ON cafe_policies(next_poll) WHERE enabled;
ALTER TABLE remote_jobs ADD COLUMN trigger_kind TEXT NOT NULL DEFAULT 'offline'
 CHECK(trigger_kind IN ('offline','cafe'));

CREATE OR REPLACE FUNCTION remote_cancel(a UUID) RETURNS VOID LANGUAGE plpgsql AS $$
BEGIN
 PERFORM pg_advisory_xact_lock(73940128);
 UPDATE remote_accounts SET epoch=epoch+1,updated_at=now() WHERE id=a;
 UPDATE remote_jobs SET state='cancelled',result='authorization_or_heartbeat_changed',updated_at=now()
 WHERE account_id=a AND state IN ('queued','running');
 IF NOT EXISTS(SELECT 1 FROM remote_grants WHERE account_id=a AND enabled)
 AND NOT EXISTS(SELECT 1 FROM cafe_policies WHERE account_id=a AND enabled) THEN
  UPDATE remote_accounts SET credential=NULL,credential_state='deleted',credential_version=credential_version+1 WHERE id=a;
 END IF;
END $$;

-- Disabling a platform user also revokes independent cloud consent.
CREATE FUNCTION cafe_on_user_disable() RETURNS TRIGGER LANGUAGE plpgsql AS $$
DECLARE a UUID;
BEGIN
 IF NEW.disabled THEN
  PERFORM pg_advisory_xact_lock(73940128);
  UPDATE cafe_policies SET enabled=false,revision=revision+1,started_at=NULL,
   observed_state='unknown',poll_lease=NULL,poll_until=NULL,updated_at=now()
   WHERE account_id IN (SELECT id FROM remote_accounts WHERE user_id=NEW.id);
  FOR a IN SELECT id FROM remote_accounts WHERE user_id=NEW.id LOOP
   PERFORM remote_cancel(a);
   UPDATE remote_accounts SET credential=NULL,credential_state='deleted',credential_version=credential_version+1 WHERE id=a;
  END LOOP;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER cafe_user_disable AFTER UPDATE OF disabled ON users
 FOR EACH ROW EXECUTE FUNCTION cafe_on_user_disable();
