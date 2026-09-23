-- Existing rows and legacy API calls remain LeiGod grants.
ALTER TABLE devices ADD COLUMN etalien_revision BIGINT NOT NULL DEFAULT 0;
ALTER TABLE remote_accounts ADD COLUMN provider TEXT NOT NULL DEFAULT 'leigod'
 CHECK(provider IN ('leigod','etalien'));
ALTER TABLE remote_accounts ADD CONSTRAINT remote_accounts_provider_pair UNIQUE(id,provider);
ALTER TABLE remote_grants ADD COLUMN provider TEXT NOT NULL DEFAULT 'leigod'
 CHECK(provider IN ('leigod','etalien'));
ALTER TABLE remote_grants DROP CONSTRAINT remote_grants_pkey;
ALTER TABLE remote_grants ADD PRIMARY KEY(device_id,provider);
ALTER TABLE remote_grants ADD FOREIGN KEY(account_id,provider)
 REFERENCES remote_accounts(id,provider);

CREATE FUNCTION remote_revoke_provider(d UUID,p TEXT) RETURNS VOID LANGUAGE plpgsql AS $$
DECLARE a UUID; r BIGINT;
BEGIN
 PERFORM pg_advisory_xact_lock(73940128);
 IF p='leigod' THEN
  UPDATE devices SET remote_revision=remote_revision+1 WHERE id=d RETURNING remote_revision INTO r;
 ELSIF p='etalien' THEN
  UPDATE devices SET etalien_revision=etalien_revision+1 WHERE id=d RETURNING etalien_revision INTO r;
 ELSE RAISE EXCEPTION 'unknown provider';
 END IF;
 UPDATE remote_grants SET enabled=false,revision=r,armed_at=NULL,updated_at=now()
 WHERE device_id=d AND provider=p RETURNING account_id INTO a;
 IF a IS NOT NULL THEN PERFORM remote_cancel(a); END IF;
END $$;

CREATE OR REPLACE FUNCTION remote_revoke_device(d UUID) RETURNS VOID LANGUAGE plpgsql AS $$
BEGIN
 PERFORM remote_revoke_provider(d,'leigod');
 PERFORM remote_revoke_provider(d,'etalien');
END $$;
