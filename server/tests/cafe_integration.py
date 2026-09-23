"""Real disposable PostgreSQL and loopback providers; never a real accelerator."""
import http.cookiejar,json,os,secrets,subprocess,time,urllib.error,urllib.request
BASE=os.environ.get('BASE_URL','http://127.0.0.1:3089')
assert BASE.startswith('http://127.0.0.1:')
assert os.environ['LEIGOD_TEST_ORIGIN']=='http://127.0.0.1:3091'
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
 def login(self,u,p):
  self.call('/login',{'username':u,'password':p});m=self.call('/me');self.csrf=m['csrf'];return m['id']
admin=Client();admin.login(os.environ['TEST_ADMIN_USERNAME'],os.environ['TEST_ADMIN_PASSWORD'])
def user(n):
 c=Client();name=f'cafe-{nonce}-{n}';pw=secrets.token_hex(16)
 admin.call('/admin/users',{'username':name,'display_name':'Cafe QA','password':pw});uid=c.login(name,pw);return c,uid
owner,uid=user(1);other,other_uid=user(2)
def wait(fn,seconds=30):
 until=time.monotonic()+seconds
 while time.monotonic()<until:
  v=fn()
  if v:return v
  time.sleep(.25)
 raise AssertionError('cafe worker did not reach expected state')
def ready():sql("UPDATE remote_service SET warmup_until=now()-interval '1 second',ingress_ok=true,blocked=false,last_tick=now();")
def mock(token,port=3091,**kw):
 urllib.request.urlopen(urllib.request.Request(f'http://127.0.0.1:{port}/control',data=json.dumps({'token':token,**kw}).encode(),headers={'Content-Type':'application/json'})).read()
def stats(token,port=3091):return json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/'))[token]
def check(s):print('PASS',s,flush=True)
class Account:
 def __init__(self,client=owner,kind='leigod'):
  self.client=client;self.kind=kind;self.port=3093 if kind=='etalien' else 3091
  b=client.call('/devices/register',{'installation_key':secrets.token_hex(32),'name':'Cafe fixture','version':'qa'})
  self.device=b['device_id'];self.bearer=b['device_token'];self.seq=0;self.rev=0;self.token='cafe-'+secrets.token_hex(12)
  self.beat()
  if kind=='etalien':mock(self.token,self.port,state=2,id=secrets.randbelow(1000000000)+10000)
  else:mock(self.token,account=self.token)
  path='/device/remote'+('/etalien' if kind=='etalien' else '')+'/authorize'+('?provider=etalien' if kind=='etalien' else '')
  r=client.call(path,{'account_token':self.token,'revision':0,'run_generation':1,'device_id':'a'*32,'paused_state':2},token=self.bearer);self.rev=r['revision']
  self.id=sql(f"SELECT account_id FROM remote_grants WHERE device_id='{self.device}' AND provider='{kind}';")
 def beat(self):
  p={'sequence':self.seq,'run_generation':1,'version':'qa','game_running':True,'account_action':'keep','remote_revision':self.rev if self.kind=='leigod' else 0,'etalien_revision':self.rev if self.kind=='etalien' else 0};self.seq+=1
  return self.client.call('/device/heartbeat',p,token=self.bearer)
 def view(self):return next(x for x in self.client.call('/remote')['cafe'] if x['id']==self.id)
 def set(self,enabled=True,hours=24,status=200,revision=None,client=None):
  p={'enabled':enabled,'revision':self.view()['revision'] if revision is None else revision}
  if hours is not None:p['max_hours']=hours
  return (client or self.client).call('/remote/cafe/'+self.id,p,status)
 def poll(self):sql(f"UPDATE cafe_policies SET next_poll=now() WHERE account_id='{self.id}';")
 def running(self):
  mock(self.token,self.port,**({'state':0} if self.kind=='etalien' else {'paused':False}))
  self.poll();wait(lambda:self.view()['observed_state']=='running' and self.view()['started_at'])
 def expire(self,hours=25):
  sql(f"UPDATE cafe_policies SET started_at=now()-interval '{hours} hours',observed_at=now(),observed_state='running',next_poll=now()+interval '60 seconds' WHERE account_id='{self.id}';")
 def calls(self):return stats(self.token,self.port)['pause_calls']
 def off_device(self):
  self.client.call('/devices/'+self.device+'/remote/disable'+('?provider=etalien' if self.kind=='etalien' else ''),{})

ready();a=Account();v=a.view();assert not v['enabled'] and v['max_hours']==24
assert other.call('/remote')['cafe']==[]
a.set(client=other,status=404)
csrf=owner.csrf;owner.csrf=None;a.set(status=403,revision=0);owner.csrf=csrf
a.set(hours=0,status=400);a.set(hours=169,status=400)
a.set(hours=None);assert a.view()['max_hours']==24
a.set(revision=0,status=409)
check('explicit opt-in, 24h default, duration bounds, ownership, CSRF and revision checks')
wait(lambda:a.view()['started_at']);start=a.view()['started_at']
# Cafe mode needs no first armed heartbeat, yet old offline policy cannot fire.
sql(f"UPDATE remote_grants SET armed_at=now(),last_seen=now()-interval '125 seconds' WHERE account_id='{a.id}';")
time.sleep(6);assert a.calls()==0
epoch=sql(f"SELECT epoch FROM remote_accounts WHERE id='{a.id}';")
a.beat();assert sql(f"SELECT epoch FROM remote_accounts WHERE id='{a.id}';")==epoch
assert a.view()['started_at']==start
a.set(hours=48);assert a.view()['started_at']==start
a.expire(25);time.sleep(6);assert a.calls()==0
a.set(hours=24);wait(lambda:a.calls()==1);wait(lambda:a.view()['observed_state']=='paused')
assert a.view()['started_at'] is None and a.view()['job_state']=='confirmed'
time.sleep(6);assert a.calls()==1
check('home disconnect suppressed; heartbeats do not reset timer; edit keeps start; one confirmed pause at limit')
a.running();assert a.view()['started_at']!=start and a.calls()==1
mock(a.token,paused=True);a.poll();wait(lambda:a.view()['observed_state']=='paused');assert a.view()['started_at'] is None
a.running();second=a.view()['started_at']
sql(f"UPDATE cafe_policies SET started_at=now()-interval '23 hours',observed_at=now()-interval '6 minutes' WHERE account_id='{a.id}';")
a.poll();wait(lambda:sql(f"SELECT started_at>now()-interval '1 minute' FROM cafe_policies WHERE account_id='{a.id}';")=='t')
assert sql(f"SELECT started_at>now()-interval '1 minute' FROM cafe_policies WHERE account_id='{a.id}';")=='t'
check('manual pause ends cycle; resume starts new cycle; long unknown gap starts conservatively')
a.off_device();assert a.view()['enabled'] and a.view()['credential']=='valid'
a.expire();ready();wait(lambda:a.calls()==2)
a.set(enabled=False);assert a.view()['credential']=='deleted'
a.set(status=409)
check('independent cloud consent survives final device revoke; cloud disable deletes last credential')

# Cancelling during a delayed query must fence both observation and pause writes.
b=Account();b.set();wait(lambda:b.view()['started_at']);b.beat();b.expire()
mock(b.token,info_delay=3);ready();before=stats(b.token)['info_calls']
wait(lambda:stats(b.token)['info_calls']>before);b.set(enabled=False)
time.sleep(5);assert b.calls()==0 and not b.view()['enabled']
assert sql(f"SELECT armed_at IS NULL FROM remote_grants WHERE account_id='{b.id}';")=='t'
mock(b.token,info_delay=0);b.off_device()
check('disable during slow query cancels write; old home grants require fresh heartbeat after leaving cafe')

# Account-level provider routing remains independent.
et=Account(kind='etalien');et.set();wait(lambda:et.view()['observed_state']=='paused')
et.running();et.off_device();et.expire();ready();wait(lambda:et.calls()==1);wait(lambda:et.view()['observed_state']=='paused')
et.set(enabled=False)
check('ETAlien uses its verified paused-state adapter without any installed/online device')

# Invalid/unknown results never masquerade as an ended session or a pause.
c=Account();c.set();wait(lambda:c.view()['started_at'])
mock(c.token,mode='unknown');c.poll();wait(lambda:c.view()['observed_state']=='unknown');assert c.calls()==0
mock(c.token,mode='expired');c.poll();wait(lambda:c.view()['credential']=='reauthorize');assert c.calls()==0
c.set(enabled=False);c.off_device()
check('unknown upstream state defers; expired credential requires reauthorization without pause')

# Durable polling leases prevent overlapping reads and recover after a worker dies.
e=Account();e.set();wait(lambda:e.view()['started_at'])
sql(f"UPDATE cafe_policies SET next_poll=now(),poll_lease=gen_random_uuid(),poll_until=now()+interval '60 seconds' WHERE account_id='{e.id}';")
before=stats(e.token)['info_calls'];time.sleep(6);assert stats(e.token)['info_calls']==before
sql(f"UPDATE cafe_policies SET poll_until=now()-interval '1 second' WHERE account_id='{e.id}';")
wait(lambda:stats(e.token)['info_calls']>before)
wait(lambda:sql(f"SELECT poll_lease IS NULL FROM cafe_policies WHERE account_id='{e.id}';")=='t')
# A stale delayed poll must not resurrect disabled consent or overwrite its fields.
mock(e.token,info_delay=3);before=stats(e.token)['info_calls'];e.poll()
wait(lambda:stats(e.token)['info_calls']>before);e.set(enabled=False)
time.sleep(5);assert e.view()['started_at'] is None and not e.view()['enabled'] and e.calls()==0
mock(e.token,info_delay=0);e.off_device()
check('poll leases recover after expiry; delayed observation cannot resurrect disabled policy')

# A disabled platform user cannot leave a cloud policy or usable credential behind.
d=Account(client=other);d.set();wait(lambda:d.view()['started_at'])
admin.call('/admin/users/'+other_uid+'/status',{'disabled':True})
assert sql(f"SELECT NOT enabled FROM cafe_policies WHERE account_id='{d.id}';")=='t'
assert sql(f"SELECT credential IS NULL FROM remote_accounts WHERE id='{d.id}';")=='t'
assert d.calls()==0
visible=json.dumps(owner.call('/remote'))+json.dumps(owner.call('/events'))
assert a.token not in visible and b.token not in visible and 'credential_version' not in visible
check('user disable revokes independent cloud policy; API and audit contain no credential secrets')
