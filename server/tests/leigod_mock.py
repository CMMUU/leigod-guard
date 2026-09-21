"""Loopback-only deterministic provider simulator. Never calls a real account API."""
import http.server,json,threading,time,urllib.parse
state={};lock=threading.Lock()
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def send(self,v,code=200):
  data=json.dumps(v).encode();self.send_response(code);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers()
  try:self.wfile.write(data)
  except (BrokenPipeError,ConnectionResetError):pass
 def do_GET(self):
  with lock:self.send({k:{'paused':v.get('paused',False),'pause_calls':v.get('pause_calls',0),'info_calls':v.get('info_calls',0)} for k,v in state.items()})
 def do_POST(self):
  parsed=urllib.parse.urlsplit(self.path)
  if parsed.path=='/control':
   p=json.loads(self.rfile.read(int(self.headers.get('Content-Length',0))))
   with lock:state.setdefault(p['token'],{'account':p['token'],'paused':False,'pause_calls':0,'info_calls':0}).update(p)
   self.send({'ok':True});return
  token=urllib.parse.parse_qs(parsed.query).get('account_token',[''])[0]
  with lock:
   v=state.get(token)
   if not v:self.send({'code':400006});return
   snapshot=dict(v);mode=v.get('mode','normal')
   if parsed.path.endswith('/info'):v['info_calls']+=1
   if mode=='expired':self.send({'code':400006});return
   if parsed.path.endswith('/pause'):
    v['pause_calls']+=1
    if mode!='no_apply':v['paused']=True
    snapshot=dict(v)
  if parsed.path.endswith('/info'):
   time.sleep(snapshot.get('info_delay',0))
   if mode=='unknown':self.send({'code':0,'data':{'user_id':snapshot['account'],'pause_status_id':2}})
   elif mode=='missing_id':self.send({'code':0,'data':{'pause_status_id':0}})
   else:self.send({'code':0,'data':{'user_id':snapshot['account'],'pause_status_id':1 if snapshot['paused'] else 0}})
  elif parsed.path.endswith('/pause'):
   if mode=='timeout_apply':time.sleep(11)
   self.send({'code':0})
  else:self.send({'code':404},404)
http.server.ThreadingHTTPServer(('127.0.0.1',3091),Handler).serve_forever()
