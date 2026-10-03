"""Disposable MySQL fixture access; never connect to a production address."""
import os
import re
import uuid
from urllib.parse import urlsplit, unquote
import pymysql


def connect():
    url = urlsplit(os.environ['DATABASE_URL'])
    assert url.scheme == 'mysql' and url.hostname in ('127.0.0.1', 'localhost')
    assert url.path == '/guard_test', 'Tests require the disposable guard_test database'
    return pymysql.connect(host=url.hostname, port=url.port or 3306,
                           user=unquote(url.username), password=unquote(url.password),
                           database='guard_test', charset='utf8mb4', autocommit=True,
                           init_command="SET time_zone='+00:00'")


def sql(query):
    # Fixture IDs originate from the API, which exposes canonical UUID strings.
    # Production SQL uses bound sqlx UUID values, not this fixture convenience.
    query = re.sub(r"'([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})'",
                   r"UUID_TO_BIN('\1')", query)
    def cell(value):
        if value is None:
            return ''
        if isinstance(value, bytes):
            return str(uuid.UUID(bytes=value)) if len(value) == 16 else value.hex()
        return str(value)
    with connect() as conn, conn.cursor() as cur:
        cur.execute(query)
        return '\n'.join('|'.join(map(cell, row)) for row in cur.fetchall()) if cur.description else ''
