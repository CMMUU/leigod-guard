#!/usr/bin/env python3
"""Create new private configuration; never overwrite an existing deployment."""
import argparse
import os
from pathlib import Path
import secrets

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=Path, default=Path.cwd())
parser.add_argument('--docker', action='store_true', help='Linux root: key owner becomes image UID 10001')
args = parser.parse_args()
if args.docker and (os.name != 'posix' or os.geteuid() != 0):
    parser.error('--docker requires root on the Linux Docker host')
target = args.output.resolve()
env = target / 'server.env'
key = target / 'secrets' / 'remote.key'
if env.exists() or env.is_symlink() or key.exists() or key.is_symlink():
    parser.error('existing server.env or remote.key: refusing to overwrite; keep your existing secrets')
target.mkdir(parents=True, exist_ok=True)
key.parent.mkdir(mode=0o700, exist_ok=True)
if key.parent.is_symlink():
    parser.error('secrets directory must not be a symlink')
key.parent.chmod(0o700)
template = Path(__file__).with_name('server.env.example').read_text()
content = template.replace('REPLACE_DATABASE_PASSWORD', secrets.token_hex(32))
content = content.replace('REPLACE_EMAIL_SECRET', secrets.token_hex(32))
old_umask = os.umask(0o077)
try:
    with env.open('x') as f:
        f.write(content)
    with key.open('xb') as f:
        f.write(secrets.token_bytes(32))
    if args.docker:
        os.chown(key, 10001, 10001)
finally:
    os.umask(old_umask)
print('Created private server.env and 32-byte secrets/remote.key. Remote execution is OFF.')
