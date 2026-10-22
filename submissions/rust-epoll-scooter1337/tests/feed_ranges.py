import argparse,base64,hashlib,hmac,http.client,json,os,pathlib,shutil,signal,socket,sqlite3,subprocess,tempfile,time
p=argparse.ArgumentParser();p.add_argument('--seed',required=True);p.add_argument('--server',required=True);a=p.parse_args();n=0
sql='SELECT p.id,p.body,p.created_at,u.username,(SELECT count(*) FROM likes l WHERE l.post_id=p.id) FROM posts p JOIN users u ON u.id=p.user_id ORDER BY p.created_at DESC,p.id DESC LIMIT 20'
def request(method,path,body=None,headers={}):
 c=http.client.HTTPConnection('127.0.0.1',13005,timeout=10);c.request(method,path,body,headers);r=c.getresponse();result=(r.status,json.loads(r.read()));c.close();return result
def b64(b):return base64.urlsafe_b64encode(b).rstrip(b'=').decode()
for name,ids in [('dense-reordered',[300000+2*i for i in range(20)]),('sparse',[1000+20000*i for i in range(20)])]:
 with tempfile.TemporaryDirectory(prefix='feed-ranges-') as d:
  db=pathlib.Path(d)/'feed.db';shutil.copyfile(a.seed,db)
  with sqlite3.connect(db) as c:
   for i,k in enumerate(ids):c.execute('UPDATE posts SET created_at=? WHERE id=?',(f'2030-01-01T00:00:{(i*7)%20:02}.000Z',k))
   c.execute('UPDATE posts SET body=? WHERE id=?',('quote " slash \\ controls \n\t café 🍊',ids[0]))
   expected=[dict(zip(['id','body','created_at','author','like_count'],r)) for r in c.execute(sql)]
   assert len(expected)==20
  c.close()
  env=os.environ|{'SQLITE_PATH':str(db),'JWT_SECRET':'twelve-dollar-challenge','HOST':'127.0.0.1','PORT':'13005'}
  with open(pathlib.Path(d)/'server.log','w') as log:
   server=subprocess.Popen([a.server],env=env,stdout=log,stderr=log)
   try:
    deadline=time.monotonic()+60
    while time.monotonic()<deadline:
     if server.poll() is not None:raise RuntimeError('server exited during startup')
     try:
      if request('GET','/health')[0]==200:break
     except OSError:time.sleep(.1)
    else:raise RuntimeError('startup failed')
    assert request('GET','/feed')==(200,{'posts':expected});n+=1
    post=expected[0]
    assert request('GET',f'/posts/{post["id"]}')==(200,{'post':post});n+=1
    h=b64(b'{"alg":"HS256"}');payload=b64(json.dumps({'sub':'50000','username':'feed-test','exp':time.time()+3600},separators=(',',':')).encode());sig=b64(hmac.new(b'twelve-dollar-challenge',(h+'.'+payload).encode(),hashlib.sha256).digest())
    status,liked=request('POST',f'/posts/{post["id"]}/like',headers={'Authorization':'Bearer '+h+'.'+payload+'.'+sig})
    assert status in [200,201];post['like_count']+=int(status==201)
    assert request('GET','/feed')==(200,{'posts':expected});n+=1
    print('ok',name,'feed order, escaping, post parity and live likes',flush=True)
   except:
    log.flush();print(pathlib.Path(log.name).read_text(),flush=True);raise
   finally:server.kill();server.wait()
print('PASSED',n,'range-feed checks',flush=True)
