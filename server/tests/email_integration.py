"""Isolated email/device contract tests. Only a local mock mailbox and disposable DB."""
import concurrent.futures, hashlib, http.cookiejar, json, os, re, secrets, subprocess, time, urllib.error, urllib.parse, urllib.request
BASE=os.environ.get('BASE_URL','http://127.0.0.1:3089')
assert BASE.startswith('http://127.0.0.1:')
nonce=secrets.token_hex(5);checks=[]
def sql(text):
 args=json.loads(os.environ['TEST_PSQL']) if 'TEST_PSQL' in os.environ else ['psql',os.environ['DATABASE_URL'],'-v','ON_ERROR_STOP=1','-At']
 r=subprocess.run(args,input=text,text=True,capture_output=True,check=True)
 return r.stdout.strip()
class Client:
 def __init__(self):
  self.jar=http.cookiejar.CookieJar();self.http=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar));self.csrf=None;self.ip=secrets.token_hex(12);self.headers=None
 def call(self,path,body=None,status=200,headers=None):
  h={'Origin':BASE,'X-Real-IP':self.ip}
  if body is not None:h['Content-Type']='application/json'
  if self.csrf:h['X-CSRF-Token']=self.csrf
  h.update(headers or {})
  req=urllib.request.Request(BASE+'/api'+path,data=None if body is None else json.dumps(body).encode(),headers=h)
  try:r=self.http.open(req,timeout=20)
  except urllib.error.HTTPError as e:r=e
  self.headers=r.headers;data=json.loads(r.read());assert r.code==status,(path,r.code,status,data)
  return data
 def identity(self):
  user=self.call('/me');self.csrf=user['csrf'];return user
 def send(self,email):
  challenge=self.call('/email/code',{'email':email});assert 'code' not in challenge
  msg=json.load(urllib.request.urlopen('http://127.0.0.1:3090/?'+urllib.parse.urlencode({'email':email})))
  code=re.search(r'\b[0-9]{6}\b',msg['text']).group(0)
  return {'email':email,'request_id':challenge['request_id'],'code':code}
def check(name):checks.append(name);print('PASS',name)
a=Client();admin=Client();anon=Client()
admin.call('/login',{'username':os.environ['TEST_ADMIN_USERNAME'],'password':os.environ['TEST_ADMIN_PASSWORD']});admin.identity()
a.call('/email/code',{'email':'bad'},status=400)
a.call('/email/code',{'email':'a@example.invalid'},status=403,headers={'Origin':'https://foreign.invalid'})
email=f'ordinary-{nonce}@example.invalid';login=a.send(email)
a.call('/email/code',{'email':email},status=429)
assert sql(f"SELECT count(*) FROM users WHERE email='{email}';")=='0'
a.call('/email/login',dict(login,remember=True));assert 'Max-Age=2592000' in a.headers['Set-Cookie']
u=a.identity();assert u['role']=='user' and u['username']==email and u['password_enabled'] is False
expiry=float(sql(f"SELECT extract(epoch FROM expires_at-now()) FROM sessions WHERE user_id='{u['id']}';"));assert 2591900<expiry<=2592000
anon.call('/email/login',login,status=401);a.call('/admin/users',status=403)
check('verified email creates ordinary user only; remembered cookie/database TTL 30 days; one-time use')
# A legacy administrator username that looks like an email is never claimed by registration.
legacy=f'legacy-{nonce}@example.invalid';password=secrets.token_urlsafe(24)
legacy_id=admin.call('/admin/users',{'username':legacy,'display_name':'legacy','password':password})['id']
sql(f"UPDATE users SET role='admin' WHERE id='{legacy_id}';")
b=Client();b.call('/email/login',b.send(legacy));assert 'Max-Age=86400' in b.headers['Set-Cookie']
bu=b.identity();assert bu['role']=='user' and bu['id']!=legacy_id
check('email registration never takes over a legacy email-shaped administrator; default TTL 24 hours')
wrong=Client();data=wrong.send(f'wrong-{nonce}@example.invalid');bad='000000' if data['code']!='000000' else '000001'
for _ in range(5):wrong.call('/email/login',dict(data,code=bad),status=401)
wrong.call('/email/login',data,status=401)
assert sql(f"SELECT attempts FROM email_challenges WHERE email='{data['email']}';")=='5'
expired=Client();data=expired.send(f'expired-{nonce}@example.invalid');sql(f"UPDATE email_challenges SET expires_at=now()-interval '1 second' WHERE email='{data['email']}';")
expired.call('/email/login',data,status=401)
check('five incorrect attempts lock the challenge; expired codes rejected')
resend=Client();old=resend.send(f'resend-{nonce}@example.invalid');sql(f"UPDATE email_challenges SET requested_at=now()-interval '61 seconds' WHERE email='{old['email']}';")
new=resend.send(old['email']);resend.call('/email/login',old,status=401);resend.call('/email/login',new)
failed=Client();failure=f'reject-{nonce}@example.invalid';failed.call('/email/code',{'email':failure},status=503)
assert sql(f"SELECT count(*) FROM email_challenges WHERE email='{failure}';")=='0'
check('resend invalidates old challenge; provider failure leaves no usable code')
race=Client();data=race.send(f'race-{nonce}@example.invalid')
def redeem(_):
 c=Client()
 try:c.call('/email/login',data);return 200
 except AssertionError as e:return e.args[0][1]
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as p:statuses=sorted(p.map(redeem,range(2)))
assert statuses==[200,401],statuses
check('concurrent code redemption permits exactly one session')
# Stable installation identity with distinct, rotating bearer tokens.
key=secrets.token_hex(32);registration={'installation_key':key,'name':'自动绑定测试设备','version':'0.14.0'}
a.call('/devices/register',registration,status=403,headers={'X-CSRF-Token':'wrong'})
d=a.call('/devices/register',registration);again=a.call('/devices/register',registration)
assert d['device_id']==again['device_id'] and d['device_token']!=again['device_token'] and again['device_token']!=key
b.call('/devices/register',registration,status=409)
payload={'sequence':0,'game_running':None,'prepare_seconds':0,'version':'0.14.0','account_action':'link','leigod_account':{'key':hashlib.sha256(b'fixture-account').hexdigest(),'label':'雷神账号 · ***5678'}}
anon.call('/device/heartbeat',payload,status=401,headers={'Authorization':'Bearer '+d['device_token']})
anon.call('/device/heartbeat',payload,headers={'Authorization':'Bearer '+again['device_token']})
listed=a.call('/devices')['devices'];item=next(x for x in listed if x['id']==d['device_id']);assert item['game_running'] is None and item['leigod_account_label']==payload['leigod_account']['label']
assert not b.call('/devices')['devices']
visible=json.dumps(listed)+json.dumps(a.call('/events'));assert key not in visible and again['device_token'] not in visible
payload.update(sequence=1,account_action='clear',leigod_account=None)
anon.call('/device/heartbeat',payload,headers={'Authorization':'Bearer '+again['device_token']})
assert next(x for x in a.call('/devices')['devices'] if x['id']==d['device_id'])['leigod_account_label'] is None
check('automatic binding idempotent; bearer rotates; owner isolation; masked account link/clear; unknown game state')
a.call('/devices/'+d['device_id']+'/revoke',{});a.call('/devices/register',registration,status=409)
new_binding=a.call('/devices/register',dict(registration,reactivate=True));assert new_binding['device_id']==d['device_id']
code=a.call('/pairings',{})['code'];b.call('/device/pair',dict(registration,code=code),status=409)
paired=anon.call('/device/pair',dict(registration,code=code));assert paired['device_id']==d['device_id']
anon.call('/device/pair',dict(registration,code=code),status=400)
a.call('/devices/'+d['device_id']+'/unbind',{})
reowned=b.call('/devices/register',registration);assert reowned['device_id']!=d['device_id']
anon.call('/device/heartbeat',dict(payload,sequence=2),status=401,headers={'Authorization':'Bearer '+paired['device_token']})
check('revoked device cannot auto-revive; explicit rebind and one-time pairing work; unbind permits new owner without old-token access')
admin.call('/admin/users/'+u['id']+'/status',{'disabled':True});a.call('/me',status=401)
data=anon.send(email);anon.call('/email/login',data,status=401)
assert sql(f"SELECT count(*) FROM sessions WHERE user_id='{u['id']}';")=='0'
admin.call('/admin/users/'+u['id']+'/status',{'disabled':False})
check('disabled email user cannot obtain a new session')
# Persisted per-IP and per-address limits survive service workers/restarts.
limited=Client()
for _ in range(11):
 email_l=f'reject-limit-{secrets.token_hex(4)}@example.invalid'
 limited.call('/email/code',{'email':email_l},status=503 if _<10 else 429)
check('persistent per-IP delivery budget enforced')
print(json.dumps({'passed':len(checks),'checks':checks},ensure_ascii=False))
