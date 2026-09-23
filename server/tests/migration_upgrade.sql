-- Run only against the disposable CI database. The isolated schema is rolled back.
\set ON_ERROR_STOP on
BEGIN;
CREATE SCHEMA guard_upgrade_fixture;
SET LOCAL search_path TO guard_upgrade_fixture, public;
\ir ../migrations/0001_platform.sql
\ir ../migrations/0002_email_devices.sql
\ir ../migrations/0003_remote_pause.sql
INSERT INTO users(id,username,display_name,password_hash) VALUES('00000000-0000-0000-0000-000000000001','upgrade','fixture','not-a-real-hash');
INSERT INTO devices(id,user_id,name,token_hash,remote_revision) VALUES('00000000-0000-0000-0000-000000000002','00000000-0000-0000-0000-000000000001','fixture','fixture-device-token-hash',7);
INSERT INTO remote_accounts(id,user_id,provider_key,label,credential) VALUES('00000000-0000-0000-0000-000000000003','00000000-0000-0000-0000-000000000001','fixture-key','fixture',decode('aabbcc','hex'));
INSERT INTO remote_grants(device_id,account_id,revision,armed_at,last_seen) VALUES('00000000-0000-0000-0000-000000000002','00000000-0000-0000-0000-000000000003',7,now(),now());
\ir ../migrations/0004_provider_grants.sql
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM remote_grants g JOIN remote_accounts a ON a.id=g.account_id WHERE g.enabled AND g.provider='leigod' AND a.provider='leigod' AND g.revision=7 AND g.armed_at IS NOT NULL AND a.credential=decode('aabbcc','hex')) THEN
  RAISE EXCEPTION 'existing Lei grant or ciphertext changed during upgrade';
 END IF;
 PERFORM remote_revoke_provider('00000000-0000-0000-0000-000000000002','etalien');
 IF NOT EXISTS(SELECT 1 FROM remote_grants WHERE enabled AND revision=7 AND provider='leigod') THEN RAISE EXCEPTION 'ET revoke affected legacy Lei grant'; END IF;
 PERFORM remote_revoke_device('00000000-0000-0000-0000-000000000002');
 IF EXISTS(SELECT 1 FROM remote_grants WHERE enabled) OR EXISTS(SELECT 1 FROM remote_accounts WHERE credential IS NOT NULL) THEN RAISE EXCEPTION 'whole-device revoke did not clear legacy credential'; END IF;
END $$;
ROLLBACK;
\echo PASS migration 0003 to 0004 preserves existing authorization and ciphertext
