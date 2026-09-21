#!/usr/bin/env python3
"""Encrypt a verified local dump + application key, then upload with restricted SSH.
Only the age public recipient lives on Shanghai/HK. Recovery identity stays offline.
"""
import hashlib, os, pathlib, subprocess, sys
root=pathlib.Path('/opt/leigod-guard')
dump=pathlib.Path(sys.argv[1]).resolve()
if dump.parent!=root/'backups' or not dump.name.startswith('guard-') or dump.suffix!='.dump':
 raise SystemExit('unexpected backup path')
key=root/'secrets/remote.key'
if not key.is_file():raise SystemExit('remote key missing; backup not uploaded')
output=dump.with_suffix('.tar.age');temporary=output.with_suffix('.tmp')
try:
 with temporary.open('xb') as dest:
  tar=subprocess.Popen(['tar','-C',str(root),'-cf','-',str(dump.relative_to(root)),'secrets/remote.key'],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
  age=subprocess.run([str(root/'tools/age'),'-R',str(root/'secrets/backup-recipient.txt')],stdin=tar.stdout,stdout=dest,stderr=subprocess.DEVNULL)
  tar.stdout.close()
  if tar.wait()!=0 or age.returncode!=0:raise RuntimeError('backup encryption failed')
  dest.flush();os.fsync(dest.fileno())
 temporary.chmod(0o600);temporary.replace(output)
 digest=hashlib.sha256(output.read_bytes()).hexdigest()
 with output.open('rb') as payload:
  result=subprocess.run(['ssh','-F',str(root/'secrets/backup-ssh.conf'),'leigod-offsite','store',output.name],stdin=payload,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,timeout=120,check=True)
 if result.stdout.decode().strip()!=digest:raise RuntimeError('offsite digest mismatch')
 print('Encrypted offsite backup acknowledged with matching SHA-256.')
finally:
 temporary.unlink(missing_ok=True)
