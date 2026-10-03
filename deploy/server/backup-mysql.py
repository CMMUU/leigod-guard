#!/usr/bin/env python3
"""Create a private, transactionally consistent MySQL dump using DATABASE_URL."""
import argparse
import datetime as dt
import os
from pathlib import Path
import subprocess
import tempfile
from urllib.parse import urlsplit, unquote

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--directory', type=Path, required=True)
a = p.parse_args()
u = urlsplit(os.environ['DATABASE_URL'])
if u.scheme != 'mysql' or not u.path.strip('/'):
    p.error('MySQL DATABASE_URL is required')
# Option-file quoting, never shell interpolation or credentials in argv.
def quote(value):
    if any(c in value for c in '\r\n\0'):
        raise ValueError('invalid option value')
    return '"'+value.replace('\\','\\\\').replace('"','\\"')+'"'
os.umask(0o077)
a.directory.mkdir(parents=True,exist_ok=True)
output = a.directory/('guard-mysql-'+dt.datetime.now(dt.timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'.sql')
temporary = output.with_suffix('.tmp')
try:
    with tempfile.TemporaryDirectory(prefix='guard-mysql-backup-') as folder:
        options = Path(folder)/'client.cnf'
        local = u.hostname in ('127.0.0.1','localhost','::1')
        content = '[client]\n'+'\n'.join(k+'='+quote(v) for k,v in dict(host=u.hostname,port=str(u.port or 3306),user=unquote(u.username),password=unquote(u.password)).items())
        content += '\nssl-mode='+('PREFERRED' if local else 'VERIFY_IDENTITY')+'\n'
        if os.environ.get('MYSQL_SSL_CA'):
            content += 'ssl-ca='+quote(os.environ['MYSQL_SSL_CA'])+'\n'
        options.write_text(content)
        with temporary.open('xb') as dest:
            result = subprocess.run(['mysqldump','--defaults-extra-file='+str(options),
                '--single-transaction','--no-tablespaces','--set-gtid-purged=OFF','--hex-blob',
                u.path.strip('/')], stdout=dest, stderr=subprocess.DEVNULL)
            if result.returncode or dest.tell() == 0:
                raise RuntimeError('dump failed')
            dest.flush();os.fsync(dest.fileno())
        # Never overwrite an existing backup, even within the same second.
        os.link(temporary,output)
    print('MySQL backup saved: '+str(output))
finally:
    temporary.unlink(missing_ok=True)
