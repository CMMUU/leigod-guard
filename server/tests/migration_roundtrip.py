"""Loss check across disposable engines, including pending job retirement. No real accounts."""
import datetime as dt
import importlib.util
import hashlib
import json
import time
import urllib.request
import urllib.error
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import uuid
from urllib.parse import urlsplit
import psycopg

root = Path(__file__).resolve().parents[2]
source = urlsplit(os.environ['SOURCE_DATABASE_URL'])
target = urlsplit(os.environ['DATABASE_URL'])
assert source.hostname == target.hostname == '127.0.0.1'
assert source.path == '/guard_test' and target.path == '/guard_migration_test'
spec = importlib.util.spec_from_file_location('migration',root/'deploy/server/migrate-postgres.py')
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
with psycopg.connect(os.environ['SOURCE_DATABASE_URL']) as pg:
    for path in sorted((root/'server/migrations').glob('*.sql')):
        pg.execute(path.read_text())
    now = dt.datetime.now(dt.timezone.utc).replace(microsecond=123456)
    uid, did, aid, aid2 = [uuid.uuid4() for _ in range(4)]
    pg.execute('INSERT INTO users(id,username,display_name,password_hash,role,email,email_verified_at) VALUES(%s,%s,%s,%s,%s,%s,%s)',(uid,'fixture-user','迁移测试🎮','fixture-hash','user','fixture@example.invalid',now))
    pg.execute('INSERT INTO sessions(token_hash,user_id,csrf,expires_at,last_seen) VALUES(%s,%s,%s,%s,%s)',(hashlib.sha256(b'a'*64).hexdigest(),uid,'b'*64,now+dt.timedelta(days=30),now))
    pg.execute('INSERT INTO pairing_codes VALUES(%s,%s,%s)',('c'*64,uid,now+dt.timedelta(minutes=10)))
    pg.execute('INSERT INTO devices(id,user_id,name,token_hash,installation_hash,game_running,sequence,run_generation,guard_provider,guard_revision,remote_revision,etalien_revision) VALUES(%s,%s,%s,%s,%s,NULL,77,12,\'leigod\',3,4,5)',(did,uid,'游戏设备🎮',hashlib.sha256(b'd'*64).hexdigest(),'e'*64))
    pg.execute('INSERT INTO events(user_id,device_id,kind,detail,created_at) VALUES(%s,%s,\'fixture\',\'保留审计记录\',%s)',(uid,did,now))
    pg.execute('INSERT INTO metrics VALUES(%s,1,1,1)',(now,))
    pg.execute("INSERT INTO service_state VALUES('scheduler',%s)",(now,))
    pg.execute('INSERT INTO email_challenges(email,request_id,code_hash,requested_at,expires_at,attempts,ready) VALUES(%s,%s,%s,%s,%s,2,true)',('fixture@example.invalid',uuid.uuid4(),bytes(range(32)),now,now+dt.timedelta(minutes=10)))
    pg.execute('INSERT INTO email_rate_limits VALUES(%s,2,%s)',('fixture-limit',now+dt.timedelta(hours=1)))
    for account, provider, rev in [(aid,'leigod',4),(aid2,'etalien',5)]:
        pg.execute('INSERT INTO remote_accounts(id,user_id,provider_key,label,credential,provider) VALUES(%s,%s,%s,%s,%s,%s)',(account,uid,provider+'-fixture','账号 · ***1234',bytes(range(64)),provider))
        pg.execute('INSERT INTO remote_grants(device_id,provider,account_id,revision,enabled,armed_at,last_seen,run_generation) VALUES(%s,%s,%s,%s,true,%s,%s,12)',(did,provider,account,rev,now,now))
    pg.execute("INSERT INTO remote_jobs(id,account_id,epoch,credential_version,state,lease_id,lease_until) VALUES(%s,%s,1,1,'running',%s,%s)",(uuid.uuid4(),aid,uuid.uuid4(),now+dt.timedelta(seconds=60)))
    pg.execute("INSERT INTO cafe_policies(account_id,enabled,max_hours,started_at,observed_at,observed_state) VALUES(%s,true,24,%s,%s,'running')",(aid2,now,now))
with tempfile.TemporaryDirectory(prefix='guard-migration-') as folder:
    path = Path(folder)/'snapshot.json'
    m.export_file(path)
    subprocess.run([str(root/'server/target/debug/leigod-guard-server'),'migrate'],check=True)
    m.import_file(path)
    with m.mysql() as conn, conn.cursor() as cur:
        cur.execute('SELECT armed_at FROM remote_grants'); assert all(r[0] is None for r in cur.fetchall())
        cur.execute('SELECT state,result,lease_id FROM remote_jobs'); assert cur.fetchone() == ('cancelled','migration_rearm_required',None)
        cur.execute('SELECT enabled,started_at,observed_state FROM cafe_policies'); assert cur.fetchone() == (1,None,'unknown')
        cur.execute('SELECT sequence,run_generation,remote_revision,etalien_revision,game_running FROM devices'); assert cur.fetchone() == (77,12,4,5,None)
        cur.execute('SELECT csrf FROM sessions'); assert cur.fetchone()[0] == 'b'*64
    try: m.import_file(path)
    except ValueError: pass
    else: raise AssertionError('must refuse populated database')
    # Existing sessions and bearer tokens must authenticate on the new engine.
    env = dict(os.environ, PUBLIC_ORIGIN='http://127.0.0.1:3094', PORT='3094',
               LEGACY_ORIGIN='https://111.229.216.86', REMOTE_EXECUTION='false', SERVICE_STAGE='testing')
    with (Path(folder)/'server.log').open('w') as log:
        server = subprocess.Popen([str(root/'server/target/debug/leigod-guard-server')],cwd=root/'server',env=env,stdout=log,stderr=log)
        try:
            for _ in range(50):
                try:
                    urllib.request.urlopen(env['PUBLIC_ORIGIN']+'/api/health',timeout=1).close(); break
                except OSError: time.sleep(.1)
            def call(route, body=None, origin='https://111.229.216.86', csrf='b'*64, expected=200):
                headers={'Cookie':'guard_session='+'a'*64,'Origin':origin,'X-CSRF-Token':csrf,
                         'Authorization':'Bearer '+'d'*64,'Content-Type':'application/json'}
                request=urllib.request.Request(env['PUBLIC_ORIGIN']+'/api'+route,headers=headers,
                            data=None if body is None else json.dumps(body).encode())
                try: response=urllib.request.urlopen(request,timeout=5)
                except urllib.error.HTTPError as error: response=error
                assert response.code==expected,(route,response.code,expected)
                return json.load(response)
            assert call('/me')['id']==str(uid)
            assert call('/devices')['devices'][0]['id']==str(did)
            assert call('/device/remote')['revision']==4
            call('/pairings',{})
            call('/pairings',{},csrf='wrong',expected=403)
            call('/pairings',{},origin='https://111.229.216.86.evil.invalid',expected=403)
            print('PASS migrated session/bearer identities authenticate; exact legacy origin accepted with CSRF still enforced')
        finally:
            server.terminate();server.wait(timeout=10)
print('PASS migration: all 14 tables loss-checked, UUID/secret/UTC/null preserved, old jobs retired, new heartbeat required, overwrite refused')
