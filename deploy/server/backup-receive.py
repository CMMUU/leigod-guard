#!/usr/bin/env python3
"""HK forced-command receiver; no shell, reads, arbitrary paths, or overwrites."""
import hashlib,os,pathlib,re,sys,time
command=os.environ.get('SSH_ORIGINAL_COMMAND','')
match=re.fullmatch(r'store (guard-\d{8}T\d{6}Z\.tar\.age)',command)
if not match:raise SystemExit('unsupported backup operation')
root=pathlib.Path('/opt/leigod-guard-offsite');target=root/match.group(1)
tmp=root/(match.group(1)+'.'+str(os.getpid())+'.pending')
os.umask(0o077)
try:
 digest=hashlib.sha256();total=0
 with tmp.open('xb') as f:
  while True:
   data=sys.stdin.buffer.read(65536)
   if not data:break
   total+=len(data)
   if total>128*1024*1024:raise SystemExit('backup exceeds receiver quota')
   digest.update(data);f.write(data)
  f.flush();os.fsync(f.fileno())
 if total<64:raise SystemExit('incomplete backup')
 if target.exists():
  if hashlib.sha256(target.read_bytes()).digest()!=digest.digest():raise SystemExit('immutable backup conflict')
 else:os.link(tmp,target)
 for old in root.glob('guard-*.tar.age'):
  if old.stat().st_mtime<time.time()-14*86400:old.unlink()
 print(digest.hexdigest())
finally:
 tmp.unlink(missing_ok=True)
