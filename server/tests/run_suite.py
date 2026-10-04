"""Run API contracts against an explicitly disposable local MySQL database."""
import os
import pathlib
import signal
import subprocess
import sys
import time
import urllib.request
from db import connect

root = pathlib.Path(__file__).resolve().parents[1]
os.chdir(root)
connect().close()  # Reject any non-test database before starting a service.
env = os.environ.copy()
env.update(PUBLIC_ORIGIN='http://127.0.0.1:3089', PORT='3089',
    RESEND_API_KEY='fixture-mail-key', EMAIL_FROM='Guard <guard@example.invalid>',
    EMAIL_CODE_SECRET='disposable-ci-fixture-pepper-32-characters',
    EMAIL_TEST_ENDPOINT='http://127.0.0.1:3090/emails',
    ADMIN_USERNAME='qa-admin', ADMIN_PASSWORD='disposable-ci-admin-password',
    TEST_ADMIN_USERNAME='qa-admin', TEST_ADMIN_PASSWORD='disposable-ci-admin-password',
    REMOTE_EXECUTION='false', SERVICE_STAGE='testing')
logs = pathlib.Path(os.environ.get('TEST_LOG_DIR', '/tmp/guard-mysql-tests'))
logs.mkdir(parents=True, exist_ok=True)
binary = str(root / 'target/debug/leigod-guard-server')
children = []
def spawn(args, name):
    log = (logs / (name+'.log')).open('w')
    child = subprocess.Popen(args, env=env, stdout=log, stderr=log)
    children.append(child)
    return child

def start(name):
    child = spawn([binary], name)
    for _ in range(50):
        if child.poll() is not None:
            raise RuntimeError((logs / (name+'.log')).read_text())
        try:
            urllib.request.urlopen(env['PUBLIC_ORIGIN']+'/api/health', timeout=1).close()
            return child
        except OSError:
            time.sleep(.2)
    raise RuntimeError('Server not ready')

def test(name, extra=None):
    subprocess.run([sys.executable, '-u', 'tests/'+name+'_integration.py'], env=dict(env, **(extra or {})), check=True)

try:
    # The runner expects schema/admin to be created on a fresh database by caller.
    spawn([sys.executable, 'tests/mail_mock.py'], 'mail')
    server = start('observe')
    subprocess.run([sys.executable, '-u', 'tests/integration.py'], env=env, check=True)
    test('email')
    test('email_binding')
    server.send_signal(signal.SIGINT); server.wait(timeout=15)
    spawn([sys.executable, 'tests/leigod_mock.py'], 'leigod')
    spawn([sys.executable, 'tests/etalien_mock.py'], 'etalien')
    key = logs/'remote.key'; key.write_bytes(os.urandom(32)); key.chmod(0o600)
    env.update(REMOTE_EXECUTION='true', REMOTE_KEY_FILE=str(key),
        LEIGOD_TEST_ORIGIN='http://127.0.0.1:3091', ETALIEN_TEST_ORIGIN='http://127.0.0.1:3093')
    server = start('remote')
    for name in ('guard_selection','etalien','cafe'):
        test(name)
    test('remote', dict(TEST_SERVER_PID=str(server.pid), TEST_SERVER_BINARY=binary,
                       TEST_RESTART_LOG=str(logs/'restart.log')))
    # Remote suite terminates its restarted instance before returning.
    env['SERVICE_STAGE'] = 'maintenance'
    server = start('maintenance')
    for route in ('/me','/device/heartbeat'):
        try:
            urllib.request.urlopen(env['PUBLIC_ORIGIN']+'/api'+route, timeout=3)
            raise AssertionError('maintenance must refuse API traffic')
        except urllib.error.HTTPError as response:
            assert response.code == 503 and response.headers['Retry-After'] == '60'
    print('PASS maintenance: health available, API traffic refused with retry hint')
finally:
    for child in children:
        if child.poll() is None:
            child.terminate()
    for child in children:
        try: child.wait(timeout=10)
        except subprocess.TimeoutExpired: child.kill()
