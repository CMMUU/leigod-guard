"""Isolated PostgreSQL + mock provider tests. Never target a public service."""
import concurrent.futures,hashlib,http.cookiejar,json,os,secrets,subprocess,time,urllib.request,urllib.error
BASE=os.environ.get('BASE_URL','http://127.0.0.1:3089');assert BASE.startswith('http://127.0.0.1:')
assert os.environ.get('LEIGOD_TEST_ORIGIN')=='http://127.0.0.1:3091'
checks=[];nonce=secrets.token_hex(4)
def sql(text):
 args=json.loads(os.environ['TEST_PSQL']) if 'TEST_PSQL' in os.environ else ['psql',os.environ['DATABASE_URL'],'-v','ON_ERROR_STOP=1','-At']
 return subprocess.run(args,input=text,text=True,capture_output=True,check=True).stdout.strip()
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
 def login(self,u,p):self.call('/login',{'username':u,'password':p});me=self.call('/me');self.csrf=me['csrf'];return me
admin=Client();admin.login(os.environ['TEST_ADMIN_USERNAME'],os.environ['TEST_ADMIN_PASSWORD'])
assert admin.call('/health')['remote_execution'] is True
users=[]
for n in range(3):
 c=Client();name=f'remote-{nonce}-{n}';password=secrets.token_hex(16);uid=admin.call('/admin/users',{'username':name,'display_name':'Remote QA','password':password})['id'];c.login(name,password);users.append((c,uid))
a,uid=users[0];b,buid=users[1];c,cuid=users[2]
def check(name):checks.append(name);print('PASS',name,flush=True)
def mock(token,**kw):
 data={'token':token};data.update(kw);urllib.request.urlopen(urllib.request.Request('http://127.0.0.1:3091/control',data=json.dumps(data).encode(),headers={'Content-Type':'application/json'})).read()
def stats(token):return json.load(urllib.request.urlopen('http://127.0.0.1:3091/'))[token]
def ready():sql("UPDATE remote_service SET warmup_until=now()-interval '1 second',ingress_ok=true,blocked=false,reason='ready',last_tick=now();")
def wait(fn,seconds=25):
 deadline=time.monotonic()+seconds
 while time.monotonic()<deadline:
  value=fn()
  if value:return value
  time.sleep(.4)
 raise AssertionError('timed out waiting for isolated worker')
class Device:
 def __init__(self,client=a):
  self.client=client;self.binding=client.call('/devices/register',{'installation_key':secrets.token_hex(32),'name':'QA remote','version':'qa-remote'});self.id=self.binding['device_id'];self.token=self.binding['device_token'];self.seq=0;self.run=1;self.rev=0
 def status(self):
  s=self.client.call('/device/remote',token=self.token);self.rev=s['revision'];return s
 def beat(self,prepare=0,status=200,run=None,revision=None,action='keep'):
  payload={'sequence':self.seq,'run_generation':self.run if run is None else run,'remote_revision':self.rev if revision is None else revision,'game_running':None,'prepare_seconds':prepare,'version':'qa','account_action':action};self.seq+=1
  return self.client.call('/device/heartbeat',payload,status,token=self.token)
 def authorize(self,token,status=200,refresh=False):
  r=self.client.call('/device/remote/authorize',{'account_token':token,'revision':self.rev,'run_generation':self.run,'refresh':refresh},status,token=self.token)
  if status==200:self.rev=r['revision']
  return r
 def off(self):
  s=self.client.call('/device/remote/disable',{'revision':self.rev},token=self.token);self.rev=s['revision'];return s
 def stale(self,prepare=-1):sql(f"UPDATE remote_grants SET last_seen=now()-interval '125 seconds',prepare_until=now()+interval '{prepare} seconds' WHERE device_id='{self.id}';")
 def jobs(self):return json.loads(sql(f"SELECT coalesce(json_agg(json_build_object('state',j.state,'result',j.result,'attempts',j.attempts)),'[]') FROM remote_jobs j JOIN remote_grants g ON g.account_id=j.account_id WHERE g.device_id='{self.id}';"))
 def terminal(self,state):return any(j['state']==state for j in self.jobs())
 def account(self):return sql(f"SELECT account_id FROM remote_grants WHERE device_id='{self.id}';")
ready();d=Device();d.beat();assert d.status()['enabled'] is False
t='fixture-'+nonce;mock(t,account='123456');d.authorize(t);assert d.status()['protection']=='awaiting_heartbeat';d.stale();time.sleep(6);assert stats(t)['pause_calls']==0
check('explicit opt-in and first current-revision heartbeat required')
# Provider-derived ownership; no client supplied identity can take over another tenant.
x=Device(b);x.beat();x.authorize(t,status=409);assert not x.status()['enabled']
missing='missing-'+nonce;mock(missing,mode='missing_id');x.authorize(missing,status=400)
check('canonical provider identity required; cross-owner enrollment rejected')
cipher=sql(f"SELECT encode(credential,'hex') FROM remote_accounts WHERE id='{d.account()}';");assert t.encode().hex() not in cipher and len(cipher)>56
visible=json.dumps(a.call('/remote'))+json.dumps(admin.call('/remote'))+json.dumps(a.call('/events'));assert t not in visible and cipher not in visible and 'credential_version' not in visible
assert not b.call('/remote')['grants']
check('encrypted credentials; tenant read isolation and API secret redaction')
d.beat();oldrev=d.rev;d.beat(revision=oldrev-1,status=409);d.run=2;d.beat();d.beat(run=1,status=409)
check('replayed grant versions and previous client runs rejected')
# Same account on second device: one online device (even unknown game state) protects all.
e=Device();e.beat();e.authorize(t);e.beat();d.stale();time.sleep(6);assert not d.jobs();e.stale(90);time.sleep(6);assert not d.jobs()
check('account aggregation blocks pause for fresh peers or preparation windows')
e.stale();ready();wait(lambda:d.terminal('confirmed'));assert stats(t)['pause_calls']==1
time.sleep(6);assert stats(t)['pause_calls']==1 and len(d.jobs())==1
check('all devices offline: one durable task, provider confirmation, no repeated episode')
# Heartbeat re-arms new episode; already paused is queried, not sent again.
d.beat();d.stale();ready();wait(lambda:len(d.jobs())==2 and d.terminal('confirmed'));wait(lambda:len([j for j in d.jobs() if j['state']=='confirmed'])==2);assert stats(t)['pause_calls']==1
check('new heartbeat re-arms; already-paused provider avoids duplicate pause')
# Stopping final grant deletes current cipher; other devices remain enabled.
d.off();assert e.status()['enabled'];e.off();assert sql(f"SELECT credential IS NULL FROM remote_accounts WHERE id='{d.account()}';")=='t'
check('device-scoped revoke preserves peers; final revoke deletes active credentials')
# Web disable cannot be undone by token refresh; disabling before in-flight auth commits advances revision.
f=Device(c);f.beat();tf='race-'+nonce;mock(tf,account='race-account');f.status();rev=f.rev;mock(tf,info_delay=2)
with concurrent.futures.ThreadPoolExecutor() as pool:
 future=pool.submit(f.authorize,tf,409);time.sleep(.4);f.off();future.result()
mock(tf,info_delay=0);assert not f.status()['enabled'];f.authorize(tf);f.beat();c.call('/devices/'+f.id+'/remote/disable',{});f.authorize(tf,status=409,refresh=True);assert not f.status()['enabled']
check('disable cancels in-flight authorization; web revoke cannot auto-resurrect')
# Request timed out after applying; post-query confirms without another pause.
g=Device(b);g.beat();tg='timeout-'+nonce;mock(tg,account='timeout-account',mode='timeout_apply');g.authorize(tg);g.beat();g.stale();ready();wait(lambda:g.terminal('confirmed'),35);assert stats(tg)['pause_calls']==1;g.off()
check('provider timeout after applying is resolved by query, not blind retry')
# Fresh heartbeat arrives while preflight provider query is in-flight.
h=Device(c);h.beat();th='recover-'+nonce;mock(th,account='recover-account');h.authorize(th);h.beat();mock(th,info_delay=3);initial=stats(th)['info_calls'];h.stale();ready();wait(lambda:stats(th)['info_calls']>initial);h.beat();time.sleep(4);assert stats(th)['pause_calls']==0;h.off()
check('heartbeat recovery during preflight cancels unsent pause')
# Expired credential terminal, no pause; rotation verifies and changes version.
i=Device(b);i.beat();ti='expire-'+nonce;mock(ti,account='expired-account');i.authorize(ti);i.beat();mock(ti,mode='expired');i.stale();ready();wait(lambda:i.terminal('reauthorize'));assert i.status()['credential']=='reauthorize' and stats(ti)['pause_calls']==0;i.off()
check('expired credential stops execution and requests client reauthorization')
# Finite retry window, no successful HTTP is mistaken for applied state.
j=Device(c);j.beat();tj='no-apply-'+nonce;mock(tj,account='no-apply-account',mode='no_apply');j.authorize(tj);j.beat();j.stale();ready()
for attempt in range(3):
 wait(lambda:stats(tj)['pause_calls']>=attempt+1)
 wait(lambda:sql(f"SELECT count(*) FROM remote_jobs WHERE account_id='{j.account()}' AND state IN ('queued','unconfirmed');")=='1')
 sql(f"UPDATE remote_jobs SET next_attempt=now() WHERE account_id='{j.account()}' AND state='queued';")
wait(lambda:j.terminal('unconfirmed'));assert stats(tj)['pause_calls']==3;time.sleep(6);assert stats(tj)['pause_calls']==3;j.off()
check('API acceptance is not confirmation; finite retries and episode deduplication')
# Cipher tampering is terminal and never forwarded to the provider.
k=Device(b);k.beat();tk='cipher-'+nonce;mock(tk,account='cipher-account');k.authorize(tk);k.beat();sql(f"UPDATE remote_accounts SET credential=set_byte(credential,15,get_byte(credential,15)#1) WHERE id='{k.account()}';");k.stale();ready();wait(lambda:k.terminal('reauthorize'));assert stats(tk)['pause_calls']==0;k.off()
check('cipher tampering fails closed with reauthorization required')
# Rebinding and user revocation trigger atomic cancellation and credential erasure.
l=Device(c);l.beat();tl='revoke-'+nonce;mock(tl,account='revoke-account');l.authorize(tl);l.beat();c.call('/devices/'+l.id+'/revoke',{});assert sql(f"SELECT enabled FROM remote_grants WHERE device_id='{l.id}';")=='f'
admin.call('/admin/users/'+cuid+'/status',{'disabled':True});assert sql(f"SELECT count(*) FROM remote_grants g JOIN devices d ON d.id=g.device_id WHERE d.user_id='{cuid}' AND g.enabled;")=='0'
check('device/user revocation cancels durable grants atomically')
# Five simultaneous account losses trip circuit breaker; acknowledgement requires new heartbeat.
# Separate fresh user avoids intentional authorization rate limiter.
muser=Client();mname='mass-'+nonce;mp=secrets.token_hex(20);admin.call('/admin/users',{'username':mname,'password':mp,'display_name':'Mass QA'});muser.login(mname,mp)
mass=[]
for n in range(5):
 q=Device(muser);q.beat();tq=f'mass-{nonce}-{n}';mock(tq,account=tq);q.authorize(tq);q.beat();mass.append((q,tq))
ids=','.join("'"+q.id+"'" for q,_ in mass)
sql(f"UPDATE remote_grants SET last_seen=now()-interval '125 seconds',prepare_until=now()-interval '1 second' WHERE device_id IN ({ids});")
ready();wait(lambda:admin.call('/remote')['service']['state']=='blocked');assert all(stats(tq)['pause_calls']==0 for _,tq in mass)
admin.call('/admin/remote/acknowledge',{});assert sql("SELECT count(*) FROM remote_grants WHERE enabled AND armed_at IS NOT NULL;")=='0'
for q,_ in mass:q.off()
check('mass disconnect suppresses batch; explicit recovery requires new heartbeats')
# Startup/ingress grace blocks otherwise eligible tasks.
n=Device(b);n.beat();tn='warmup-'+nonce;mock(tn,account='warmup-account');n.authorize(tn);n.beat();n.stale();sql("UPDATE remote_service SET warmup_until=now()+interval '120 seconds',blocked=false,ingress_ok=true;");time.sleep(6);assert stats(tn)['pause_calls']==0;n.off()
check('restart/ingress reconnect observation window suppresses old offline tasks')
print(json.dumps({'passed':len(checks),'checks':checks},ensure_ascii=False))
