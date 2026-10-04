"""Password-account email binding: isolated MySQL and mock email only."""
import concurrent.futures, hashlib, http.cookiejar, json, os, re, secrets, subprocess, time, urllib.error, urllib.parse, urllib.request
BASE=os.environ.get('BASE_URL','http://127.0.0.1:3089')
assert BASE.startswith('http://127.0.0.1:')
nonce=secrets.token_hex(5);checks=[]
from db import sql
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
admin=Client();admin.call('/login',{'username':os.environ['TEST_ADMIN_USERNAME'],'password':os.environ['TEST_ADMIN_PASSWORD']});admin.identity()
anon=Client()
def account(label,role='user'):
 username=f'bind-{label}-{nonce}';password=secrets.token_urlsafe(24)
 uid=admin.call('/admin/users',{'username':username,'display_name':label,'password':password})['id']
 if role=='admin':sql(f"UPDATE users SET role='admin' WHERE id='{uid}'")
 c=Client();c.call('/login',{'username':username,'password':password});u=c.identity()
 return c,uid,username,password

def binding(c,email):
 response=c.call('/account/email/code',{'email':email})
 assert response['retry_after']==60 and response['expires_in']==600 and 'code' not in response
 message=json.load(urllib.request.urlopen('http://127.0.0.1:3090/?'+urllib.parse.urlencode({'email':email.lower().strip()})))
 assert '绑定邮箱' in message['subject'] and '绑定到当前已登录的平台账号' in message['text']
 return dict(email=email,request_id=response['request_id'],code=re.search(r'\b[0-9]{6}\b',message['text']).group(0))

def age(uid):sql(f"UPDATE email_binding_challenges SET requested_at=UTC_TIMESTAMP(6)-INTERVAL 61 SECOND WHERE user_id='{uid}'")

owner,uid,username,password=account('owner')
email=f'bound-{nonce}@example.invalid'
anon.call('/account/email/code',{'email':email},status=401)
owner.call('/account/email/code',{'email':email},status=403,headers={'X-CSRF-Token':'wrong'})
owner.call('/account/email/code',{'email':email},status=403,headers={'Origin':'https://foreign.invalid'})
owner.call('/account/email/code',{'email':'bad'},status=400)
proof=binding(owner,' '+email.upper()+' ')
owner.call('/account/email/code',{'email':email},status=429)
anon.call('/email/code',{'email':email},status=429)
anon.call('/email/login',proof,status=401)
check('binding requires login/origin/CSRF; normalized email; shared 60-second cooldown; binding code cannot log in')
other,other_id,_,other_password=account('other')
other.call('/account/email/bind',dict(proof,current_password=other_password),status=400)
owner.call('/account/email/bind',dict(proof,current_password='wrong-password'),status=400)
owner.call('/account/email/bind',dict(proof,current_password=password),status=403,headers={'X-CSRF-Token':'wrong'})
assert sql(f"SELECT email FROM users WHERE id='{uid}'") == ''
check('binding proof is scoped to its account and requires the current password and CSRF')
registration={'installation_key':secrets.token_hex(32),'name':'binding fixture','version':'test'}
device=owner.call('/devices/register',registration)
before=sql(f"SELECT username,password_hash,role FROM users WHERE id='{uid}'")
sessions=sql(f"SELECT token_hash,csrf,expires_at FROM sessions WHERE user_id='{uid}' ORDER BY token_hash")
count=sql('SELECT count(*) FROM users')
owner.call('/account/email/bind',dict(proof,current_password=password))
u=owner.identity()
assert u['id']==uid and u['email']==email and u['email_verified'] and u['login_username']==username and u['password_enabled']
assert sql(f"SELECT username,password_hash,role FROM users WHERE id='{uid}'")==before
assert sql(f"SELECT token_hash,csrf,expires_at FROM sessions WHERE user_id='{uid}' ORDER BY token_hash")==sessions
assert sql('SELECT count(*) FROM users')==count
assert owner.call('/devices')['devices'][0]['id']==device['device_id']
assert sql(f"SELECT count(*) FROM email_binding_challenges WHERE user_id='{uid}'")=='0'
owner.call('/account/email/bind',dict(proof,current_password=password),status=409)
owner.call('/account/email/code',{'email':f'replace-{nonce}@example.invalid'},status=409)
password_login=Client();password_login.call('/login',{'username':username,'password':password});assert password_login.identity()['id']==uid
email_login=Client();email_login.call('/email/login',dict(email_login.send(email),remember=True));assert 'Max-Age=2592000' in email_login.headers['Set-Cookie']
assert email_login.identity()['id']==uid and email_login.call('/devices')['devices'][0]['id']==device['device_id']
other.call('/account/email/code',{'email':email},status=409)
check('binding retains identity/password/role/sessions/devices; original password and remembered email login reach the same account; no overwrite')
# Login challenges must not be repurposed into binding challenges.
login_proof=anon.send(f'login-only-{nonce}@example.invalid')
other.call('/account/email/bind',dict(login_proof,current_password=other_password),status=400)
check('login verification code cannot authorize email binding')
# Resends invalidate the previous proof; failed delivery creates no usable proof.
resender,rid,_,rp=account('resend')
old=binding(resender,f'resend-bind-{nonce}@example.invalid');age(rid)
new=binding(resender,old['email'])
resender.call('/account/email/bind',dict(old,current_password=rp),status=400)
sql(f"UPDATE email_binding_challenges SET expires_at=UTC_TIMESTAMP(6)-INTERVAL 1 SECOND WHERE user_id='{rid}'")
resender.call('/account/email/bind',dict(new,current_password=rp),status=400)
age(rid)
new=binding(resender,old['email']);bad='000000' if new['code']!='000000' else '000001'
for _ in range(5):resender.call('/account/email/bind',dict(new,current_password=rp,code=bad),status=400)
resender.call('/account/email/bind',dict(new,current_password=rp),status=400)
assert sql(f"SELECT attempts FROM email_binding_challenges WHERE user_id='{rid}'")=='5'
age(rid)
resender.call('/account/email/code',{'email':f'reject-binding-{nonce}@example.invalid'},status=503)
assert sql(f"SELECT count(*) FROM email_binding_challenges WHERE user_id='{rid}'")=='0'
check('resend invalidates old proof; expired/five-error proofs rejected; mail failure deletes pending proof')
# Pending login proofs cannot become a password-account login after binding.
privileged,aid,aname,ap=account('admin','admin');aemail=f'admin-bound-{nonce}@example.invalid'
old_login=anon.send(aemail)
sql(f"UPDATE email_challenges SET requested_at=UTC_TIMESTAMP(6)-INTERVAL 61 SECOND WHERE email='{aemail}'")
admin_proof=binding(privileged,aemail)
privileged.call('/account/email/bind',dict(admin_proof,current_password=ap))
anon.call('/email/login',old_login,status=401)
admin_email=Client();admin_email.call('/email/login',admin_email.send(aemail))
au=admin_email.identity();assert au['id']==aid and au['role']=='admin'
admin_email.call('/admin/users')
check('verified administrator email logs into the same administrator; pre-binding login codes invalidated')
# A registration taking the email before confirmation must win without merging.
contender,cid,_,cp=account('occupied');ce=f'occupied-bind-{nonce}@example.invalid'
claim=binding(contender,ce);age(cid)
new_owner=Client();new_owner.call('/email/login',new_owner.send(ce));new_id=new_owner.identity()['id']
contender.call('/account/email/bind',dict(claim,current_password=cp),status=409)
assert sql(f"SELECT email FROM users WHERE id='{cid}'")=='' and new_id!=cid
check('email claimed before confirmation is rejected atomically without merging or overwriting users')
# Two valid simultaneous confirmations have exactly one winner.
racer,race_id,_,race_pw=account('race');race_proof=binding(racer,f'race-bind-{nonce}@example.invalid')
race_cookie='; '.join(c.name+'='+c.value for c in racer.jar)
def redeem(_):
 c=Client()
 try:
  c.call('/account/email/bind',dict(race_proof,current_password=race_pw),headers={'Cookie':race_cookie,'X-CSRF-Token':racer.csrf});return 200
 except AssertionError as e:return e.args[0][1]
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:statuses=sorted(pool.map(redeem,range(2)))
assert statuses==[200,409],statuses
assert sql(f"SELECT count(*) FROM events WHERE user_id='{race_id}' AND kind='email_bound'")=='1'
check('concurrent confirmation binds once and records one audit event')
# Disabled users and revoked sessions cannot complete a pending binding.
disabled,did,_,dp=account('disabled');disabled_proof=binding(disabled,f'disabled-bind-{nonce}@example.invalid')
admin.call('/admin/users/'+did+'/status',{'disabled':True})
disabled.call('/account/email/bind',dict(disabled_proof,current_password=dp),status=401)
assert sql(f"SELECT email FROM users WHERE id='{did}'")==''
revoked,vid,_,vp=account('revoked');revoked_proof=binding(revoked,f'revoked-bind-{nonce}@example.invalid')
revoked.call('/logout',{})
revoked.call('/account/email/bind',dict(revoked_proof,current_password=vp),status=401)
assert sql(f"SELECT email FROM users WHERE id='{vid}'")==''
check('disabled account and revoked session cannot bind')
print(json.dumps({'passed':len(checks),'checks':checks},ensure_ascii=False))
