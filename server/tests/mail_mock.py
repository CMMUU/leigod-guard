"""Isolated loopback Resend-shaped mailbox. Never sends mail outside the test process."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json,threading,uuid
messages={};lock=threading.Lock()
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args): pass
 def answer(self,status,data):
  self.send_response(status);self.send_header('Content-Type','application/json');self.end_headers();self.wfile.write(json.dumps(data).encode())
 def do_POST(self):
  if self.path!='/emails':return self.answer(404,{})
  if self.headers.get('Authorization')!='Bearer fixture-mail-key':return self.answer(401,{})
  data=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
  email=data['to'][0]
  if not email.endswith('.invalid'):return self.answer(400,{'error':'test addresses only'})
  if email.startswith('reject-'):return self.answer(503,{'error':'fixture delivery failure'})
  with lock:messages[email]=data
  self.answer(200,{'id':str(uuid.uuid4())})
 def do_GET(self):
  from urllib.parse import urlparse,parse_qs
  email=parse_qs(urlparse(self.path).query).get('email',[''])[0]
  with lock:data=messages.get(email)
  self.answer(200 if data else 404,data or {})
if __name__=='__main__':ThreadingHTTPServer(('127.0.0.1',3090),Handler).serve_forever()
