#!/usr/bin/env python3
"""Black-box protocol, auth, live-read, crash-recovery and connection tests.
Run on Linux: python3 tests/check.py --seed /path/feed.db --server target/release/twelve-rust-poll
"""
import argparse,base64,concurrent.futures,hashlib,hmac,http.client,json,os,resource,select,shutil,socket,subprocess,tempfile,time
p=argparse.ArgumentParser()
p.add_argument('--seed',required=True);p.add_argument('--server',required=True)
p.add_argument('--connections',type=int,default=15000)
p.add_argument('--idle-seconds',type=int,default=66)
a=p.parse_args()
if resource.getrlimit(resource.RLIMIT_NOFILE)[0] < a.connections+64:
 p.error('raise the file-descriptor limit before running: ulimit -n 65535 (the challenge service limit)')
secret='twelve-dollar-challenge';port=3001;checks=0
def check(ok,label):
 global checks
 assert ok,label
 checks+=1
 print('ok',label,flush=True)
def b64(b):return base64.urlsafe_b64encode(b).rstrip(b'=').decode()
def token(payload,header=None):
 h=b64(json.dumps(({'alg':'HS256','typ':'JWT'} if header is None else header),separators=(',',':')).encode());b=b64(json.dumps(payload,separators=(',',':'),ensure_ascii=False).encode())
 s=b64(hmac.new(secret.encode(),(h+'.'+b).encode(),hashlib.sha256).digest())
 return h+'.'+b+'.'+s
def auth(**changes):
 payload=dict(sub='1',username='golden_ember_1',exp=time.time()+3600);payload.update(changes)
 return 'Bearer '+token(payload)
def req(method,path,body=None,authorization=None):
 c=http.client.HTTPConnection('127.0.0.1',port,timeout=10)
 headers={'Content-Type':'application/json'}
 if authorization:headers['Authorization']=authorization
 c.request(method,path,body,headers);r=c.getresponse();data=r.read();c.close()
 return r.status,json.loads(data)
def sock(source=None):
 return socket.create_connection(('127.0.0.1',port),timeout=10,source_address=(source,0) if source else None)
def wire(s,count=1):
 # Buffered reader is created once per socket consumer and retains pipelined bytes.
 f=s.makefile('rb');out=[]
 for _ in range(count):
  line=f.readline();assert line.startswith(b'HTTP/1.1'),line
  status=int(line.split()[1]);length=None
  while True:
   line=f.readline()
   if line==b'\r\n':break
   assert line,line
   if line.lower().startswith(b'content-length:'):length=int(line.split(b':',1)[1])
  assert length is not None
  data=f.read(length);assert len(data)==length
  out.append((status,json.loads(data)))
 f.close();return out
with tempfile.TemporaryDirectory(prefix='twelve-tests-') as d:
 db=d+'/feed.db';shutil.copyfile(a.seed,db)
 env=dict(os.environ,SQLITE_PATH=db,JWT_SECRET=secret,HOST='127.0.0.1',PORT=str(port))
 log=open(d+'/server.log','w')
 server=None;sockets=[]
 def start():
  global server
  server=subprocess.Popen([os.path.abspath(a.server)],env=env,stdout=log,stderr=log)
  for _ in range(100):
   try:
    if req('GET','/health')[0]==200:return
   except OSError:pass
   time.sleep(.1)
  raise RuntimeError('server did not start')
 try:
  start()
  for path in ['/posts/like','/posts//like']:
   check(req('POST',path,None)[0]==401,'missing like ID checks auth first '+path)
   check(req('POST',path,None,auth())==(400,{'error':'invalid post id'}),'missing like ID '+path)
   check(req('GET','/health')[0]==200,'healthy after missing like ID '+path)
  for sub in [0,1,True,'0','-1','1.5','abc',None]:
   status,data=req('POST','/posts','{"body":"x"}',auth(sub=sub))
   check((status,data)==(401,{'error':'invalid token payload'}),'invalid sub '+repr(sub))
  for name in [0,True,None,{},[]]:
   status,data=req('POST','/posts','{"body":"x"}',auth(username=name))
   check((status,data)==(401,{'error':'invalid token payload'}),'invalid username '+repr(name))
  for changes in [dict(exp=0),dict(exp='3000000000'),dict(exp=None),dict(nbf=time.time()+3600),dict(nbf='bad')]:
   status,data=req('POST','/posts','{"body":"x"}',auth(**changes))
   check((status,data)==(401,{'error':'invalid or expired token'}),'invalid token time '+repr(changes))
  for header in [dict(alg='none'),dict(alg='HS512'),dict(alg=0),{}]:
   t='Bearer '+token(dict(sub='1',username='name',exp=time.time()+3600),header)
   check(req('POST','/posts','{"body":"x"}',t)[0]==401,'invalid alg '+repr(header))
  # Unicode code points, trim, quoting and escaped JWT usernames.
  for body in ['\ufeff\u00a0 café ✓ \u2028','😀'*500,'quote " slash \\ tab\t newline\n\x00end']:
   username='a"b\\c café 😀'
   status,data=req('POST','/posts',json.dumps({'body':body}),auth(username=username))
   check(status==201,'Unicode/escaping create')
   check(data['post']['body']==body.strip('\ufeff \u00a0\u2028'),'Unicode trim/round trip')
   check(data['post']['author']==username,'escaped JWT username')
  check(req('POST','/posts',json.dumps({'body':'😀'*501}),auth())[0]==400,'501 Unicode code points')
  for body in [b'{"body":"\xff"}',b'{"body":"\\ud800"}',b'{"body":"x"}garbage']:
   check(req('POST','/posts',body,auth())[0]==400,'malformed Unicode/JSON')
  status,post=req('POST','/posts','{"body":"concurrent likes"}',auth());post=post['post'];pid=post['id']
  with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
   replies=list(pool.map(lambda _:req('POST',f'/posts/{pid}/like',None,auth()),range(16)))
  check(sum(r[0]==201 for r in replies)==1 and sum(r[0]==200 for r in replies)==15,'16 duplicate likes serialized')
  check(req('GET',f'/posts/{pid}')[1]['post']['like_count']==1,'fresh count after concurrent likes')
  # Fragmented requests and chunked JSON, with trailers and chunk extensions.
  s=sock();raw=(f'POST /posts HTTP/1.1\r\nHost: x\r\nAuthorization: {auth()}\r\nContent-Length: 16\r\n\r\n'+ '{"body":"split"}').encode()
  for offset in range(0,len(raw),7):s.sendall(raw[offset:offset+7])
  check(wire(s)[0][0]==201,'fragmented headers/body');s.close()
  s=sock();raw=(f'POST /posts HTTP/1.1\r\nHost: x\r\nAuthorization: {auth()}\r\nTransfer-Encoding: chunked\r\n\r\n').encode()+b'9;test=yes\r\n{"body":"\r\n7\r\nchunk"}\r\n0\r\nX-Trailer: ok\r\n\r\n'
  for offset in range(0,len(raw),13):s.sendall(raw[offset:offset+13])
  check(wire(s)[0][0]==201,'fragmented chunked body/extensions/trailers');s.close()
  for headers in ['Content-Length: 1\r\nContent-Length: 2\r\n','Content-Length: 5\r\nTransfer-Encoding: chunked\r\n']:
   s=sock();s.sendall(('POST /posts HTTP/1.1\r\nHost: x\r\n'+headers+'\r\nx').encode())
   check(wire(s)[0][0]==400,'reject ambiguous HTTP framing');s.close()
  s=sock();s.sendall(b'POST /posts HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\npartial');s.close()
  check(req('GET','/health')[0]==200,'abandoned upload')
  s=sock()
  for chunked in [False,True]:
   data=b'{"body":"continue"}'
   framing='Transfer-Encoding: chunked' if chunked else f'Content-Length: {len(data)}'
   raw=(f'POST /posts HTTP/1.1\r\nHost: x\r\nAuthorization: {auth()}\r\nExpect: 100-continue\r\n{framing}\r\n\r\n').encode()
   s.sendall(raw[:20]);s.sendall(raw[20:])
   f=s.makefile('rb');check(f.readline()==b'HTTP/1.1 100 Continue\r\n' and f.readline()==b'\r\n','100 Continue '+framing);f.close()
   first=b'3\r\n'+data[:3]+b'\r\n' if chunked else data[:3]
   rest=(f'{len(data)-3:x}\r\n'.encode()+data[3:]+b'\r\n0\r\n\r\n') if chunked else data[3:]
   s.sendall(first)
   check(not select.select([s],[],[],.1)[0],'no duplicate interim response '+framing)
   s.sendall(rest);check(wire(s)[0][0]==201,'body accepted after Continue '+framing)
  s.close()
  # Enough output to exceed the socket send buffer; delay the reader deliberately.
  s=sock();s.setsockopt(socket.SOL_SOCKET,socket.SO_RCVBUF,65536)
  raw=b'GET /feed HTTP/1.1\r\nHost: x\r\n\r\n'*1800
  s.sendall(raw);time.sleep(.2)
  replies=wire(s,1800)
  check(all(status==200 and len(data['posts'])==20 for status,data in replies),'1800 pipelined responses/backpressure');s.close()
  # A write followed by a read in the same pipeline must see the new data.
  s=sock();raw=(f'POST /posts HTTP/1.1\r\nHost: x\r\nAuthorization: {auth()}\r\nContent-Length: 21\r\n\r\n{{"body":"pipeline-x"}}GET /feed HTTP/1.1\r\nHost: x\r\n\r\n').encode()
  s.sendall(raw);r=wire(s,2)
  check(r[0][0]==201 and r[1][1]['posts'][0]==r[0][1]['post'],'pipeline read observes its committed write');s.close()
  # Record acknowledged data, SIGKILL (no destructors/checkpoint), and restart.
  status,post=req('POST','/posts','{"body":"durable before reply"}',auth());pid=post['post']['id']
  check(req('POST',f'/posts/{pid}/like',None,auth())[0]==201,'acknowledged like')
  server.kill();server.wait();start()
  recovered=req('GET',f'/posts/{pid}')
  check(recovered[0]==200 and recovered[1]['post']['body']=='durable before reply' and recovered[1]['post']['like_count']==1,'SIGKILL recovery of acknowledged post/like')
  for i in range(a.connections):
   # Distribute loopback source addresses so TIME_WAIT sockets from preceding
   # benchmark trials cannot exhaust a single local ephemeral port range.
   s=sock('127.0.0.'+str(2+i%4));s.sendall(b'GET /health HTTP/1.1\r\nHost: x\r\n\r\n')
   assert wire(s)[0][0]==200;sockets.append(s)
  check(len(sockets)==a.connections,f'{a.connections} simultaneous health-validated keep-alive sockets')
  with open(f'/proc/{server.pid}/status') as f:
   print('CONNECTION MEMORY',*[line.strip() for line in f if line.startswith(('VmRSS:','VmHWM:','RssAnon:','RssFile:'))],flush=True)
  if a.idle_seconds:
   print('waiting',a.idle_seconds,'seconds to validate original sockets',flush=True)
   time.sleep(a.idle_seconds)
   for s in sockets[:200]:
    s.sendall(b'GET /health HTTP/1.1\r\nHost: x\r\n\r\n');assert wire(s)[0][0]==200
   check(True,f'200 original sockets reused after {a.idle_seconds}s without reconnecting')
  print('PASSED',checks,'additional checks',flush=True)
 finally:
  for s in sockets:s.close()
  if server and server.poll() is None:server.kill();server.wait()
  log.close()
