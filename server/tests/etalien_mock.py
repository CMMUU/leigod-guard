"""Loopback fixture for the official identity/duration/pause protobuf protocol."""
import hashlib,http.server,json,threading,time,urllib.parse
state={};lock=threading.Lock()
def varint(n):
 out=bytearray()
 while n>127:out.append((n&127)|128);n>>=7
 out.append(n);return bytes(out)
def field(n,v):return varint(n<<3)+varint(v)
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def send(self,data,code=200):
  if not isinstance(data,bytes):data=json.dumps(data).encode()
  self.send_response(code);self.send_header('Content-Length',str(len(data)));self.end_headers()
  try:self.wfile.write(data)
  except (BrokenPipeError,ConnectionResetError):pass
 def do_GET(self):self.handle_call('GET')
 def do_POST(self):self.handle_call('POST')
 def handle_call(self,method):
  u=urllib.parse.urlsplit(self.path)
  body=self.rfile.read(int(self.headers.get('Content-Length',0)))
  if u.path=='/control':
   p=json.loads(body)
   with lock:state.setdefault(p['token'],{'id':123456,'state':2,'pause_calls':0,'profile_calls':0}).update(p)
   self.send({'ok':True});return
  if u.path=='/':
   with lock:self.send(state)
   return
  q=urllib.parse.parse_qs(u.query);canonical='&'.join(f'{k}={q[k][0]}' for k in ('nonce','ts','ver'))
  if q.get('sig')!=[hashlib.sha256(f'{method}api.et-api.com{u.path}?{canonical}'.encode()).hexdigest()] or self.headers.get('reqChannel')!='1' or 'os=2&ver=1.0.0&dvc=' not in self.headers.get('x-eta',''):
   self.send(b'',400);return
  token=self.headers.get('Authorization','')
  with lock:
   v=state.get(token)
   if not v or v.get('expired'):self.send(b'',401);return
   snap=dict(v)
   if u.path=='/account/v1/my_profile' and method=='GET':v['profile_calls']+=1;result=field(1,snap['id']) if snap['id'] else b''
   elif u.path=='/v2/account/remain/duration' and method=='POST':result=field(1,600)+field(3,int(time.time()))+field(4,snap['state'])
   elif u.path=='/v2/account/update/pause/state' and method=='POST':
    if body!=field(1,2):self.send(b'',400);return
    v['pause_calls']+=1
    if not snap.get('no_apply'):v['state']=2
    result=b''
   else:self.send(b'',404);return
  time.sleep(snap.get('delay',0));self.send(result)
http.server.ThreadingHTTPServer(('127.0.0.1',3093),Handler).serve_forever()
