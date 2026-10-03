"""Single-device guard switch regression; loopback mocks and disposable DB only."""
import concurrent.futures
import http.cookiejar
import json
import os
import secrets
import subprocess
import time
import urllib.error
import urllib.request

BASE = os.environ.get('BASE_URL', 'http://127.0.0.1:3089')
assert BASE.startswith('http://127.0.0.1:')
assert os.environ['LEIGOD_TEST_ORIGIN'] == 'http://127.0.0.1:3091'
assert os.environ['ETALIEN_TEST_ORIGIN'] == 'http://127.0.0.1:3093'
nonce = secrets.token_hex(5)


from db import sql


class Client:
    def __init__(self):
        self.http = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.csrf = None

    def call(self, path, body=None, status=200, token=None):
        h = {'Origin': BASE}
        if self.csrf:
            h['X-CSRF-Token'] = self.csrf
        if token:
            h['Authorization'] = 'Bearer ' + token
        if body is not None:
            h['Content-Type'] = 'application/json'
        try:
            r = self.http.open(urllib.request.Request(BASE + '/api' + path,
                data=None if body is None else json.dumps(body).encode(), headers=h), timeout=35)
        except urllib.error.HTTPError as e:
            r = e
        data = json.loads(r.read())
        assert r.code == status, (path, r.code, status, data)
        return data

    def login(self, user, password):
        self.call('/login', {'username': user, 'password': password})
        self.csrf = self.call('/me')['csrf']


admin = Client()
admin.login(os.environ['TEST_ADMIN_USERNAME'], os.environ['TEST_ADMIN_PASSWORD'])
password = secrets.token_hex(16)
username = 'selection-' + nonce
admin.call('/admin/users', {'username': username, 'display_name': 'Selection QA', 'password': password})
c = Client()
c.login(username, password)


def device():
    return c.call('/devices/register', {'installation_key': secrets.token_hex(32),
        'name': 'Selection fixture', 'version': 'qa'})


def request(d, path, body=None, status=200):
    return c.call(path, body, status, d['device_token'])


def remote(d, kind):
    return request(d, '/device/remote?provider=' + kind)


def heartbeat(d, seq, selection=None, status=200):
    p = {'sequence': seq, 'run_generation': 1, 'version': 'qa', 'account_action': 'keep',
        'remote_revision': remote(d, 'leigod')['revision'],
        'etalien_revision': remote(d, 'etalien')['revision']}
    if selection:
        p.update(guard_provider=selection['provider'], guard_revision=selection['revision'])
    return request(d, '/device/heartbeat', p, status)


def select(d, provider, revision, run=1, status=200):
    return request(d, '/device/guard', {'provider': provider, 'revision': revision,
        'run_generation': run}, status)


def mock(token, kind, **kw):
    port = 3091 if kind == 'leigod' else 3093
    urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/control',
        data=json.dumps({'token': token, **kw}).encode(), headers={'Content-Type': 'application/json'})).read()


def authorize(d, kind, token, status=200, revision=None):
    return request(d, '/device/remote/authorize?provider=' + kind, {
        'account_token': token, 'device_id': 'a' * 32, 'paused_state': 2,
        'run_generation': 1, 'revision': remote(d, kind)['revision'] if revision is None else revision}, status)


def check(text):
    print('PASS', text, flush=True)


# Block dispatch while asserting transactions; external calls use only local mocks.
sql("UPDATE remote_service SET blocked=true,reason='selection_fixture';")
d, peer = device(), device()
heartbeat(d, 0)
heartbeat(peer, 0)
lei, et = 'selection-lei-' + nonce, 'selection-et-' + nonce
mock(lei, 'leigod', account='selection-' + nonce)
mock(et, 'etalien', id=int(nonce, 16) + 1000000000000, state=2)
authorize(d, 'leigod', lei)
authorize(d, 'etalien', et)
authorize(peer, 'leigod', lei)
heartbeat(d, 1)
sql(f"UPDATE remote_grants SET last_seen=UTC_TIMESTAMP(6)-INTERVAL 125 SECOND WHERE device_id='{d['device_id']}';")
assert request(d, '/device/guard')['provider'] is None
# A legacy target may have a queued offline episode; retain consent but discard it.
legacy_aid = sql(f"SELECT account_id FROM remote_grants WHERE device_id='{d['device_id']}' AND provider='leigod';")
sql(f"INSERT INTO remote_jobs(id,account_id,epoch,credential_version,state) SELECT UUID_TO_BIN(UUID()),id,epoch,credential_version,'queued' FROM remote_accounts WHERE id='{legacy_aid}';")
s1 = select(d, 'leigod', 0)
assert sql(f"SELECT count(*) FROM remote_grants WHERE device_id='{d['device_id']}' AND provider='leigod' AND armed_at IS NOT NULL AND last_seen>UTC_TIMESTAMP(6)-INTERVAL 5 SECOND;") == '1'
assert sql(f"SELECT count(*) FROM remote_jobs WHERE account_id='{legacy_aid}' AND state='queued';") == '0'
assert s1['committed'] and s1['revision'] == 1
assert remote(d, 'leigod')['enabled'] and not remote(d, 'etalien')['enabled']
heartbeat(d, 2, s1)
s2 = select(d, 'etalien', s1['revision'])
assert s2['committed'] and s2['other_devices'] == 1 and not remote(d, 'leigod')['enabled']
assert not remote(d, 'etalien')['enabled'], 'selection must not authorize target'
assert remote(peer, 'leigod')['enabled'], 'peer device consent must remain intact'
heartbeat(d, 3, s1, 409)
heartbeat(d, 3, None, 409)
heartbeat(d, 3, s2)
authorize(d, 'leigod', lei, 409)
select(d, 'leigod', s1['revision'], status=409)
check('legacy opt-in, selected-only authorization, stale heartbeat/CAS rejected, peer preserved')

# Admission before a slow upstream validation cannot resurrect a deselected grant.
old_revision = remote(d, 'etalien')['revision']
mock(et, 'etalien', delay=2)
with concurrent.futures.ThreadPoolExecutor() as pool:
    future = pool.submit(authorize, d, 'etalien', et, 409, old_revision)
    time.sleep(.4)
    s3 = select(d, 'leigod', s2['revision'])
    future.result()
mock(et, 'etalien', delay=0)
assert s3['committed'] and not remote(d, 'etalien')['enabled']
check('in-flight authorization cannot restore the old provider')

# Independent account-level cafe policy requires owner action, never silent removal.
aid = sql(f"SELECT account_id FROM remote_grants WHERE device_id='{d['device_id']}' AND provider='leigod';")
sql(f"INSERT INTO cafe_policies(account_id,enabled) VALUES('{aid}',true) ON DUPLICATE KEY UPDATE enabled=true;")
blocked = select(d, 'etalien', s3['revision'])
assert not blocked['committed'] and blocked['reason'] == 'cafe_mode'
assert sql(f"SELECT enabled FROM cafe_policies WHERE account_id='{aid}';") =='1'
sql(f"UPDATE cafe_policies SET enabled=false WHERE account_id='{aid}';")
# An already sent pause cannot be recalled. Wait for its durable lease to drain.
sql(f"INSERT INTO remote_jobs(id,account_id,epoch,credential_version,state,lease_id,lease_until) SELECT UUID_TO_BIN(UUID()),id,epoch,credential_version,'running',UUID_TO_BIN(UUID()),UTC_TIMESTAMP(6)+INTERVAL 80 SECOND FROM remote_accounts WHERE id='{aid}';")
blocked = select(d, 'etalien', s3['revision'])
assert not blocked['committed'] and blocked['reason'] == 'in_flight'
assert request(d, '/device/guard')['revision'] == s3['revision']
sql(f"UPDATE remote_jobs SET state='cancelled',lease_id=NULL,lease_until=NULL WHERE account_id='{aid}';")
s4 = select(d, 'etalien', s3['revision'])
assert s4['committed']
# Lost response retry is idempotent; a newer process owns the next generation.
retry = select(d, 'etalien', s4['revision'], run=2)
assert retry['revision'] == s4['revision'] and retry['committed']
select(d, 'leigod', s4['revision'], run=1, status=409)
check('cafe consent and sent-job lease block switch; retries and process fencing hold')

# Release fixture grants before other integration suites run.
for item in [d, peer]:
    for kind in ['leigod', 'etalien']:
        request(item, '/device/remote/disable?provider=' + kind, {'revision': remote(item, kind)['revision']})
sql("UPDATE remote_service SET blocked=false,reason='ready',warmup_until=UTC_TIMESTAMP(6)+INTERVAL 120 SECOND;")
print('PASS single accelerator selection integration', flush=True)
