"""Isolated two-provider regression. Only fixture credentials and loopback HTTP."""
import http.cookiejar,json,os,secrets,subprocess,time,urllib.error,urllib.request
BASE=os.environ.get('BASE_URL','http://127.0.0.1:3089');assert BASE.startswith('http://127.0.0.1:')
assert os.environ['ETALIEN_TEST_ORIGIN']=='http://127.0.0.1:3093'
nonce=secrets.token_hex(5)
def sql(q):
 args=json.loads(os.environ['TEST_PSQL']) if 'TEST_PSQL' in os.environ else ['psql',os.environ['DATABASE_URL'],'-v','ON_ERROR_STOP=1','-At']
 return subprocess.run(args,input=q,text=True,capture_output=True,check=True).stdout.strip()
class Client:
 def __init__(self):self.http=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()));self.csrf=None
 def call(self,path,body=None,status=200,token=None):
  h={'Origin':BASE}
  if self.csrf:h['X-CSRF-Token']=self.csrf
  if token:h['Authorization']='Bearer '+token
  if body is not None:h['Content-Type']='application/json'
  try:r=self.http.open(urllib.request.Request(BASE+'/api'+path,data=None if body is None else json.dumps(body).encode(),headers=h),timeout=35)
  except urllib.error.HTTPError as e:r=e
  data=json.loads(r.read());assert r.code==status,(path,r.code,status,data);return data
 def login(self,u,p):self.call('/login',{'username':u,'password':p});self.csrf=self.call('/me')['csrf']
admin=Client();admin.login(os.environ['TEST_ADMIN_USERNAME'],os.environ['TEST_ADMIN_PASSWORD'])
def user(n):
 c=Client();name=f'etalien-{nonce}-{n}';pw=secrets.token_hex(16)
 admin.call('/admin/users',{'username':name,'display_name':'ET QA','password':pw});c.login(name,pw);return c
owner=user(1);other=user(2)
def mock(t,port=3093,**kw):
 urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/control',data=json.dumps({'token':t,**kw}).encode(),headers={'Content-Type':'application/json'})).read()
def stats(t):return json.load(urllib.request.urlopen('http://127.0.0.1:3093/'))[t]
def ready():sql("UPDATE remote_service SET warmup_until=now()-interval '1 second',ingress_ok=true,blocked=false,last_tick=now();")
def wait(fn):
 until=time.monotonic()+25
 while time.monotonic()<until:
  if fn():return
  time.sleep(.3)
 raise AssertionError('worker did not complete')
class Device:
 def __init__(self,c=owner):
  self.c=c;b=c.call('/devices/register',{'installation_key':secrets.token_hex(32),'name':'Two providers','version':'qa'});self.id=b['device_id'];self.token=b['device_token'];self.seq=0;self.rev={'leigod':0,'etalien':0}
 def path(self,k,action=''):return '/device/remote'+('/etalien' if k=='etalien' else '')+action+('?provider=etalien' if k=='etalien' else '')
 def status(self,k):
  r=self.c.call(self.path(k),token=self.token);self.rev[k]=r['revision'];return r
 def beat(self,legacy=False):
  p={'sequence':self.seq,'run_generation':1,'remote_revision':self.rev['leigod'],'version':'qa','account_action':'keep'};self.seq+=1
  if not legacy:p['etalien_revision']=self.rev['etalien']
  return self.c.call('/device/heartbeat',p,token=self.token)
 def auth(self,k,t,status=200,paused=2,refresh=False):
  r=self.c.call(self.path(k,'/authorize'),{'account_token':t,'device_id':'a'*32,'paused_state':paused,'revision':self.rev[k],'run_generation':1,'refresh':refresh},status,token=self.token)
  if status==200:self.rev[k]=r['revision']
  return r
 def off(self,k):
  r=self.c.call(self.path(k,'/disable'),{'revision':self.rev[k]},token=self.token);self.rev[k]=r['revision'];return r
 def aid(self,k):return sql(f"SELECT account_id FROM remote_grants WHERE device_id='{self.id}' AND provider='{k}';")
 def stale(self,k='etalien'):sql(f"UPDATE remote_grants SET last_seen=now()-interval '125 seconds',prepare_until=now()-interval '1 second' WHERE device_id='{self.id}' AND provider='{k}';")
 def confirmed(self):return sql(f"SELECT count(*) FROM remote_jobs WHERE account_id='{self.aid('etalien')}' AND state='confirmed';")=='1'
def check(t):print('PASS',t,flush=True)
d=Device();d.beat();et='et-'+nonce;lei='lei-'+nonce
mock(et,state=0);d.auth('etalien',et,status=400)
mock(et,state=2,id=0);d.auth('etalien',et,status=400)
mock(et,state=2,id=123456);d.auth('etalien',et,paused=1,status=400)
d.auth('etalien',et)
mock(lei,port=3091,account='123456');d.auth('leigod',lei);d.beat()
assert d.status('etalien')['enabled'] and d.status('leigod')['enabled']
assert d.aid('etalien')!=d.aid('leigod')
check('official identity and paused calibration required; two independent grants per physical device')
x=Device(other);x.beat();x.auth('etalien',et,status=409)
assert not x.status('etalien')['enabled'];assert not other.call('/remote')['grants']
check('verified identity prevents cross-owner authorization')
# A new token of the same account groups with the same heartbeat peers.
et2='rotation-'+nonce;mock(et2,id=123456,state=2)
e=Device();e.beat();e.auth('etalien',et2);e.beat();assert e.aid('etalien')==d.aid('etalien')
mock(et2,state=0);d.stale();ready();time.sleep(6);assert stats(et2)['pause_calls']==0
e.stale();ready();wait(d.confirmed);assert stats(et2)['pause_calls']==1
check('token rotation preserves account grouping; online peer blocks pause; offline task queries confirmation')
assert d.status('leigod')['enabled'];e.off('etalien');d.off('etalien')
assert d.status('leigod')['enabled']
assert sql(f"SELECT credential IS NULL FROM remote_accounts WHERE id='{d.aid('etalien')}';")=='t'
check('ET disable deletes final ET cipher and preserves Lei authorization')
mock(et,state=2);d.auth('etalien',et);d.beat();d.beat(legacy=True)
assert not d.status('etalien')['enabled'] and d.status('leigod')['enabled']
check('legacy heartbeat revokes unsupported ET protection while preserving Lei')
# Web control affects only the selected provider; whole-device revoke affects both.
mock(et,state=2);d.auth('etalien',et);d.beat()
owner.call('/devices/'+d.id+'/remote/disable?provider=etalien',{})
assert not d.status('etalien')['enabled'] and d.status('leigod')['enabled']
# A separate device/user avoids the deliberately low authorization rate limit.
yowner=user(3);y=Device(yowner);y.beat();yt='y-'+nonce;yl='yl-'+nonce
mock(yt,id=654321,state=2);mock(yl,port=3091,account='654321');y.auth('etalien',yt);y.auth('leigod',yl);y.beat()
yowner.call('/devices/'+y.id+'/revoke',{})
assert sql(f"SELECT count(*) FROM remote_grants WHERE device_id='{y.id}' AND enabled;")=='0'
assert sql(f"SELECT count(*) FROM remote_accounts a JOIN remote_grants g ON g.account_id=a.id WHERE g.device_id='{y.id}' AND a.credential IS NOT NULL;")=='0'
check('web per-provider disable and device-wide atomic revoke')
# Rejected identities and errors are not leaked through API or audit views.
visible=json.dumps(owner.call('/remote'))+json.dumps(owner.call('/events'))
assert et not in visible and et2 not in visible and lei not in visible
d.off('leigod')
check('credentials absent from public views and audit')
