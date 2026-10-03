#!/usr/bin/env python3
"""Offline, loss-checked PostgreSQL -> MySQL 8.4 migration. Never log row values."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import uuid
from urllib.parse import urlsplit, unquote

TABLES = ('users', 'sessions', 'pairing_codes', 'devices', 'events', 'metrics',
          'service_state', 'email_challenges', 'email_rate_limits', 'remote_accounts',
          'remote_grants', 'remote_jobs', 'remote_service', 'cafe_policies')


def encode(value):
    if isinstance(value, uuid.UUID):
        return {'uuid': str(value)}
    if isinstance(value, (bytes, memoryview)):
        return {'bytes': bytes(value).hex()}
    if isinstance(value, dt.datetime):
        return {'utc': value.replace(tzinfo=dt.timezone.utc).isoformat(timespec='microseconds')}
    if isinstance(value, bool):
        return int(value)
    return value


def decode(value):
    if isinstance(value, dict):
        if set(value) == {'uuid'}:
            return uuid.UUID(value['uuid']).bytes
        if set(value) == {'bytes'}:
            return bytes.fromhex(value['bytes'])
        if set(value) == {'utc'}:
            return dt.datetime.fromisoformat(value['utc']).astimezone(dt.timezone.utc).replace(tzinfo=None)
        raise ValueError('unknown encoded value')
    return value


def digest(rows):
    # Ordering of physical rows is irrelevant. Include nulls and type tags.
    records = sorted(json.dumps(row, sort_keys=True, ensure_ascii=False, separators=(',', ':')) for row in rows)
    return hashlib.sha256('\n'.join(records).encode()).hexdigest()


def export_file(path):
    import psycopg
    bundle = {'format': 1, 'tables': {}}
    with psycopg.connect(os.environ['SOURCE_DATABASE_URL']) as conn:
        conn.execute('SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY')
        conn.execute("SET TIME ZONE 'UTC'")
        for table in TABLES:
            with conn.cursor() as cur:
                cur.execute('SELECT * FROM '+table)
                rows = [[encode(v) for v in row] for row in cur.fetchall()]
                bundle['tables'][table] = dict(columns=[c.name for c in cur.description], rows=rows, sha256=digest(rows))
    old = os.umask(0o077)
    try:
        with path.open('x') as f:
            json.dump(bundle, f, ensure_ascii=False)
            f.flush(); os.fsync(f.fileno())
    finally:
        os.umask(old)
    print(json.dumps({'exported': {t: len(bundle['tables'][t]['rows']) for t in TABLES}}))


def mysql():
    import pymysql
    url = urlsplit(os.environ['DATABASE_URL'])
    if url.scheme != 'mysql' or not url.path.strip('/'):
        raise ValueError('MySQL DATABASE_URL required')
    tls = {}
    if url.hostname not in ('127.0.0.1', 'localhost', '::1'):
        tls = dict(ssl_verify_cert=True, ssl_verify_identity=True,
                   ssl_ca=os.environ.get('MYSQL_SSL_CA'))
    return pymysql.connect(host=url.hostname, port=url.port or 3306,
                           user=unquote(url.username), password=unquote(url.password),
                           database=url.path.strip('/'), charset='utf8mb4',
                           init_command="SET time_zone='+00:00'", **tls)


def import_file(path):
    if path.stat().st_mode & 0o077:
        raise ValueError('snapshot must have mode 0600 or stricter')
    bundle = json.loads(path.read_text())
    if bundle.get('format') != 1 or set(bundle['tables']) != set(TABLES):
        raise ValueError('snapshot table set/version mismatch')
    for table, data in bundle['tables'].items():
        if digest(data['rows']) != data['sha256']:
            raise ValueError('snapshot checksum mismatch: '+table)
    with mysql() as conn, conn.cursor() as cur:
        cur.execute('SELECT id FROM guard_lock WHERE id=1 FOR UPDATE')
        for table in TABLES:
            cur.execute('SELECT count(*) FROM `'+table+'`')
            count = cur.fetchone()[0]
            if count and not (table == 'remote_service' and count == 1):
                raise ValueError('target must be empty: '+table)
        cur.execute('DELETE FROM remote_service')
        for table in TABLES:
            data = bundle['tables'][table]
            cur.execute('SHOW COLUMNS FROM `'+table+'`')
            types = {row[0]: row[1] for row in cur.fetchall()}
            cols = data['columns']
            if set(types) != set(cols):
                raise ValueError('column mismatch: '+table)
            names = ','.join('`'+col+'`' for col in cols)
            insert = 'INSERT INTO `'+table+'` ('+names+') VALUES ('+','.join(['%s']*len(cols))+')'
            for row in data['rows']:
                cur.execute(insert, tuple(decode(value) for value in row))
            cur.execute('SELECT '+names+' FROM `'+table+'`')
            actual = []
            for row in cur.fetchall():
                actual.append([encode(uuid.UUID(bytes=v) if v is not None and types[c].lower() == 'binary(16)' else v) for c, v in zip(cols, row)])
            if digest(actual) != data['sha256']:
                raise ValueError('round-trip verification failed: '+table)
        # A cutover is not an ordinary short restart: old offline episodes must
        # never fire on arrival. Preserve consent/IDs, require fresh heartbeats.
        cur.execute("UPDATE remote_jobs SET state='cancelled',result='migration_rearm_required',lease_id=NULL,lease_until=NULL WHERE state IN ('queued','running')")
        retired = cur.rowcount
        cur.execute('UPDATE remote_accounts SET epoch=epoch+1')
        cur.execute('UPDATE remote_grants SET armed_at=NULL')
        cur.execute("UPDATE cafe_policies SET started_at=NULL,observed_at=NULL,observed_state='unknown',poll_lease=NULL,poll_until=NULL,next_poll=UTC_TIMESTAMP(6),failures=0")
        cur.execute("UPDATE remote_service SET warmup_until=UTC_TIMESTAMP(6)+INTERVAL 120 SECOND,ingress_ok=false,last_tick=UTC_TIMESTAMP(6),reason=CASE WHEN blocked THEN reason ELSE 'migration' END")
        conn.commit()
    print(json.dumps({'verified': {t: len(bundle['tables'][t]['rows']) for t in TABLES}, 'retired_jobs': retired, 'fresh_heartbeat_required': True}))


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=('export','import'))
    p.add_argument('file', type=Path)
    p.add_argument('--confirm-empty-target', action='store_true')
    args = p.parse_args()
    if args.action == 'import' and not args.confirm_empty_target:
        p.error('stop both services, initialize a fresh MySQL schema, then pass --confirm-empty-target')
    try:
        (export_file if args.action == 'export' else import_file)(args.file)
    except Exception as error:
        # DB exceptions can include credentials, source data and connection URLs.
        print('Migration failed; target transaction rolled back. Error type: '+type(error).__name__)
        raise SystemExit(1)
