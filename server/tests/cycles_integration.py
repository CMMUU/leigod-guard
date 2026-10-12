"""v2 account coordination contracts; disposable MySQL and loopback providers only."""
import concurrent.futures
import http.cookiejar
import json
import os
import secrets
import time
import urllib.error
import urllib.request
from db import sql

BASE = 'http://127.0.0.1:3089'
assert os.environ['LEIGOD_TEST_ORIGIN'] == 'http://127.0.0.1:3091'
nonce = secrets.token_hex(5)


class Client:
    def __init__(self):
        self.http = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.csrf = ''

    def call(self, path, body=None, token=None, status=200):
        headers = {'Origin': BASE, 'Content-Type': 'application/json', 'X-CSRF-Token': self.csrf}
        if token:
            headers['Authorization'] = 'Bearer ' + token
        req = urllib.request.Request(BASE+'/api'+path, headers=headers,
            data=None if body is None else json.dumps(body).encode())
        try:
            res = self.http.open(req, timeout=20)
        except urllib.error.HTTPError as e:
            res = e
        data = json.load(res)
        assert res.code == status, (path, res.code, data)
        return data

    def login(self, user, password):
        self.call('/login', dict(username=user, password=password))
        self.csrf = self.call('/me')['csrf']


admin = Client()
admin.login(os.environ['TEST_ADMIN_USERNAME'], os.environ['TEST_ADMIN_PASSWORD'])
password = secrets.token_hex(16)
admin.call('/admin/users', dict(username='cycle-'+nonce, display_name='Cycle fixture', password=password))
c = Client()
c.login('cycle-'+nonce, password)
token = 'cycle-'+nonce
urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:3091/control',
    data=json.dumps(dict(token=token, account='cycle-'+nonce, paused=False)).encode(),
    headers={'Content-Type': 'application/json'})).close()


class Device:
    def __init__(self):
        self.d = c.call('/devices/register', dict(installation_key=secrets.token_hex(32), name='Cycle fixture', version='qa'))
        self.run = 1
        self.seq = 0
        self.cycle = None
        self.grant = 0
        self.selection = None
        self.heartbeat()
        self.selection = self.req('/device/guard', dict(provider='leigod', revision=0, run_generation=1))
        self.grant = self.req('/device/remote/authorize', dict(account_token=token, revision=0, run_generation=1))['revision']
        self.heartbeat()

    def req(self, path, body=None, status=200):
        return c.call(path, body, self.d['device_token'], status)

    def heartbeat(self):
        self.seq += 1
        p = dict(sequence=self.seq, run_generation=self.run, version='qa', account_action='keep', remote_revision=self.grant)
        if self.selection:
            p.update(guard_provider='leigod', guard_revision=self.selection['revision'])
        self.req('/device/heartbeat', p)

    def send(self, op, status=200, **kw):
        self.seq += 1
        p = dict(op=op, provider='leigod', run_generation=self.run, grant_revision=self.grant,
                 sequence=self.seq, cycle_id=self.cycle, startup_seconds=180, exit_seconds=90, startup_enabled=True,
                 phase='absent', observer_age=0, executor_age=0, billing_paused=False, billing_age=0)
        p.update(kw)
        out = self.req('/device/cycle', p, status)
        if status == 200:
            self.cycle = out['cycle_id']
        return out


def check(label):
    print('PASS cycle:', label, flush=True)


def ready():
    sql("UPDATE remote_service SET blocked=false,ingress_ok=true,warmup_until=UTC_TIMESTAMP(6)-INTERVAL 1 SECOND,last_tick=UTC_TIMESTAMP(6);")


def due(account):
    ready()
    sql(f"UPDATE guard_observers SET idle_until=UTC_TIMESTAMP(6)-INTERVAL 1 SECOND,observed_at=UTC_TIMESTAMP(6),executor_at=UTC_TIMESTAMP(6) WHERE account_id='{account}';")


try:
    a = Device()
    aid = sql(f"SELECT account_id FROM remote_grants WHERE device_id='{a.d['device_id']}' AND provider='leigod';")
    a.send('status', status=409)
    a.send('enable')
    ready()
    first = a.send('observe')
    assert first['cycle_id'] and 178 <= first['remaining_seconds'] <= 181
    deadline = sql(f"SELECT idle_until FROM guard_observers WHERE device_id='{a.d['device_id']}';")
    a.send('observe')
    a.send('enable')  # Network retry must not reset the observer/deadline.
    assert deadline == sql(f"SELECT idle_until FROM guard_observers WHERE device_id='{a.d['device_id']}';")
    check('explicit consent; repeated running/enable preserves cycle and deadline')
    due(aid)
    request_id = secrets.token_hex(16)
    claimed = a.send('claim', request_id=request_id)
    action = claimed['action_id']
    assert action
    assert a.send('claim', request_id=request_id)['action_id'] == action
    assert a.send('claim', request_id=secrets.token_hex(16))['action_id'] is None
    a.send('observe', phase='running', game_generation=1)
    a.send('dispatch', status=409, action_id=action)
    check('one account permit; game start cancels an unsent permit')
    a.send('observe', phase='absent', game_generation=1)
    assert 88 <= a.send('status')['remaining_seconds'] <= 91
    a.send('observe', phase='unknown', observer_age=86400, game_generation=1)
    assert a.send('claim', request_id=secrets.token_hex(16))['action_id'] is None
    a.send('observe', game_generation=1)
    due(aid)
    old_cycle = a.cycle
    action = a.send('claim', request_id=secrets.token_hex(16))['action_id']
    a.send('dispatch', action_id=action)
    a.send('complete', action_id=action, result='unknown')
    assert a.send('claim', request_id=secrets.token_hex(16))['action_id'] is None
    a.send('complete', action_id=action, result='confirmed')
    assert a.send('status')['state'] == 'completed'
    # A delayed pre-completion running sample cannot recreate a cycle.
    assert a.send('observe', billing_age=30, game_generation=1)['cycle_id'] == old_cycle
    time.sleep(2.1)
    newer = a.send('observe', game_generation=1)
    assert newer['cycle_id'] != old_cycle and newer['remaining_seconds'] >= 178
    a.send('observe', billing_paused=True, billing_age=30, game_generation=1)
    assert a.send('status')['state'] == 'running'
    a.send('complete', action_id=action, result='confirmed')
    assert a.send('status')['state'] == 'running'
    check('unknown outcome holds writes; duplicate completion cannot finish the next cycle')
    peer = Device()  # Old protocol peer blocks v2 execution.
    due(aid)
    assert a.send('claim', request_id=secrets.token_hex(16))['action_id'] is None
    peer.send('enable')
    peer.send('observe', phase='running')
    assert a.send('claim', request_id=secrets.token_hex(16))['action_id'] is None
    peer.send('observe', phase='absent')
    due(aid)
    assert a.send('status')['state'] == 'ready'
    check('legacy peer and active game peer both prevent account-wide pause')
    sql(f"INSERT INTO cafe_policies(account_id,enabled) VALUES('{aid}',true) ON DUPLICATE KEY UPDATE enabled=true;")
    assert a.send('status')['mode'] == 'cafe'
    assert a.send('claim', request_id=secrets.token_hex(16))['action_id'] is None
    sql(f"UPDATE cafe_policies SET enabled=false WHERE account_id='{aid}';")
    check('cafe mode suppresses coordinated idle pause')
    a.send('policy', policy_revision=a.send('status')['policy_revision'], startup_seconds=240, exit_seconds=120)
    v = a.send('observe')
    assert v['startup_seconds'] == 240 and v['exit_seconds'] == 120
    revision = v['policy_revision']
    a.send('policy', policy_revision=revision, startup_seconds=240, exit_seconds=120, startup_enabled=False)
    a.send('policy', status=409, policy_revision=revision, startup_enabled=True)
    a.send('observe', phase='unknown')
    v = a.send('observe')
    assert not v['startup_enabled'] and v['remaining_seconds'] == 0
    a.send('policy', policy_revision=v['policy_revision'], startup_seconds=240, exit_seconds=120, startup_enabled=True)
    a.send('observe')
    a.send('observe', status=400, run_generation=0)
    # Observer failure with normal heartbeat is not absence.
    due(aid)
    sql(f"UPDATE guard_observers SET observed_at=UTC_TIMESTAMP(6)-INTERVAL 60 SECOND,executor_at=UTC_TIMESTAMP(6)-INTERVAL 65 SECOND WHERE account_id='{aid}';")
    assert a.send('claim', request_id=secrets.token_hex(16))['action_id'] is None
    time.sleep(6)
    stats = lambda: json.load(urllib.request.urlopen('http://127.0.0.1:3091/'))[token]
    assert stats()['pause_calls'] == 0
    # Healthy observer, stalled executor: cloud can use the same account cycle.
    due(aid)
    sql(f"UPDATE guard_observers SET executor_at=UTC_TIMESTAMP(6)-INTERVAL 65 SECOND WHERE account_id='{aid}';")
    for _ in range(30):
        if sql(f"SELECT state FROM guard_cycles WHERE id='{a.cycle}';") == 'completed':
            break
        time.sleep(1)
    assert sql(f"SELECT state FROM guard_cycles WHERE id='{a.cycle}';") == 'completed'
    assert stats()['pause_calls'] == 1
    check('stale observer cannot trigger; healthy observer with stalled executor delegates once')
    # Restarting official billing after a completed cycle needs a new observation.
    old = a.cycle
    time.sleep(2.1)
    a.send('observe', billing_paused=False)
    assert a.cycle != old
    assert a.send('status')['remaining_seconds'] >= 238

finally:
    sql(f"UPDATE remote_service SET blocked=true,reason='cycle_fixture_cleanup';")
    uid = c.call('/me')['id']
    sql(f"DELETE FROM users WHERE id='{uid}';")
    sql("UPDATE remote_service SET blocked=false,reason='ready';")

check('versioned policy and old-instance isolation; fixtures removed')
