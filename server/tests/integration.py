"""Run against a disposable database, never the production service.
BASE_URL, TEST_ADMIN_USERNAME, TEST_ADMIN_PASSWORD are supplied by the runner.
"""
import concurrent.futures, http.cookiejar, json, os, secrets, urllib.request, urllib.error

BASE=os.environ.get('BASE_URL','http://127.0.0.1:3089')
assert BASE.startswith('http://127.0.0.1:'), 'Integration tests require an isolated local service.'
checks=[]
class Client:
 def __init__(self):
  self.jar=http.cookiejar.CookieJar();self.http=urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar));self.csrf=None
 def call(self,path,body=None,status=200,headers=None):
  h={'Origin':BASE}
  if body is not None:h['Content-Type']='application/json'
  if self.csrf:h['X-CSRF-Token']=self.csrf
  if headers:h.update(headers)
  req=urllib.request.Request(BASE+'/api'+path,data=None if body is None else json.dumps(body).encode(),headers=h)
  try:r=self.http.open(req,timeout=20)
  except urllib.error.HTTPError as e:r=e
  result=json.loads(r.read());assert r.code==status,(path,r.code,status,result)
  return result
 def login(self,username,password):
  self.call('/login',{'username':username,'password':password});u=self.call('/me');self.csrf=u['csrf'];return u
def check(label):checks.append(label);print('PASS',label)
admin=Client();anon=Client()
assert anon.call('/health')['remote_execution'] is False
anon.call('/me',status=401);anon.call('/admin/overview',status=401)
check('unauthenticated requests rejected; remote execution disabled')
anon.call('/login',{'username':'missing','password':'incorrect'},status=401)
anon.call('/login',{'username':'missing','password':'incorrect'},status=403,headers={'Origin':'https://foreign.invalid'})
check('invalid login and foreign origin rejected')
admin.login(os.environ['TEST_ADMIN_USERNAME'],os.environ['TEST_ADMIN_PASSWORD'])
cookies=list(admin.jar);assert cookies and all(c.has_nonstandard_attr('HttpOnly') for c in cookies)
check('real password login, HttpOnly cookie and session identity')
nonce=secrets.token_hex(5);password=secrets.token_urlsafe(20)
a_name='qa-a-'+nonce;b_name='qa-b-'+nonce
a_id=admin.call('/admin/users',{'username':a_name,'display_name':'QA 用户 A','password':password})['id']
b_id=admin.call('/admin/users',{'username':b_name,'display_name':'QA 用户 B','password':password})['id']
admin.call('/admin/users',{'username':a_name,'display_name':'重复用户','password':password},status=409)
a=Client();b=Client();a.login(a_name,password);b.login(b_name,password)
a.call('/admin/users',status=403);a.call('/admin/overview',status=403)
check('user creation, duplicate prevention and server-enforced admin permissions')
a.call('/pairings',{},status=403,headers={'X-CSRF-Token':'wrong'})
a.call('/pairings',{},status=403,headers={'Origin':'https://foreign.invalid'})
code1=a.call('/pairings',{})['code'];code2=a.call('/pairings',{})['code']
anon.call('/device/pair',{'code':code1,'name':'old-code','version':'qa'},status=400)
pair=anon.call('/device/pair',{'code':code2,'name':'QA <设备>','version':'qa-1'})
anon.call('/device/pair',{'code':code2,'name':'replay','version':'qa'},status=400)
check('CSRF enforced, old pairing code invalidated, one-time redemption')
did=pair['device_id'];token=pair['device_token'];auth={'Authorization':'Bearer '+token}
assert not b.call('/devices')['devices']
b.call('/devices/'+did+'/revoke',{},status=404)
check('cross-user device listing and revocation denied')
payload={'sequence':0,'game_running':True,'prepare_seconds':60,'version':'qa-1'}
assert anon.call('/device/heartbeat',payload,headers=auth)['remote_execution'] is False
assert a.call('/devices')['devices'][0]['status']=='online'
anon.call('/device/heartbeat',payload,status=409,headers=auth)
payload['sequence']=1;anon.call('/device/heartbeat',payload,headers=auth)
payload['sequence']=2;payload['prepare_seconds']=601
anon.call('/device/heartbeat',payload,status=400,headers=auth)
check('device authentication, online state, replay rejection and protection bounds')
visible=json.dumps(a.call('/devices'))+json.dumps(a.call('/events'))+json.dumps(admin.call('/admin/users'))
assert token not in visible and password not in visible and 'password_hash' not in visible and 'token_hash' not in visible
check('device tokens and password hashes absent from user/admin read APIs')
a.call('/devices/'+did+'/revoke',{})
payload.update(sequence=3,prepare_seconds=0)
anon.call('/device/heartbeat',payload,status=401,headers=auth)
check('revoked device cannot report heartbeat')
racecode=b.call('/pairings',{})['code']
def redeem(_):
 c=Client();h={'Origin':BASE,'Content-Type':'application/json'}
 req=urllib.request.Request(BASE+'/api/device/pair',data=json.dumps({'code':racecode,'name':'race','version':'qa'}).encode(),headers=h)
 try:r=c.http.open(req,timeout=20)
 except urllib.error.HTTPError as e:r=e
 return r.code,json.loads(r.read())
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:results=list(pool.map(redeem,range(2)))
assert sorted(r[0] for r in results)==[200,400],results
btoken=next(r[1]['device_token'] for r in results if r[0]==200)
check('concurrent pairing redemption creates only one device')
admin.call('/admin/users/'+b_id+'/status',{'disabled':True})
b.call('/me',status=401)
anon.call('/device/heartbeat',payload,status=401,headers={'Authorization':'Bearer '+btoken})
admin.call('/admin/users/'+b_id+'/status',{'disabled':False})
anon.call('/device/heartbeat',payload,status=401,headers={'Authorization':'Bearer '+btoken})
check('disabled user sessions/devices revoked; re-enable cannot revive device token')
old_session=Client();old_session.login(a_name,password)
newpass=secrets.token_urlsafe(20)
a.call('/password',{'current_password':'wrong','new_password':newpass},status=400)
a.call('/password',{'current_password':password,'new_password':newpass})
old_session.call('/me',status=401);a.call('/me',status=401)
a.login(a_name,newpass);a.call('/logout',{});a.call('/me',status=401)
check('password change revokes all sessions; logout removes authentication')
overview=admin.call('/admin/overview');assert overview['stats']['scheduler_at'] is not None
assert isinstance(overview['metrics'],list)
check('persistent observer scheduler and real metrics available')
print(json.dumps({'passed':len(checks),'checks':checks},ensure_ascii=False))
