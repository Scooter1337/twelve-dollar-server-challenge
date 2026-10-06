#!/usr/bin/env python3
"""Compare two Rust submission revisions using the prepared benchmark environment."""
import argparse, fcntl, hashlib, io, json, pathlib, re, shlex, statistics, subprocess, tarfile, tempfile, time, uuid
HERE=pathlib.Path(__file__).resolve().parent
p=argparse.ArgumentParser()
p.add_argument('--base-ref',default='fbdbe2b')
p.add_argument('--candidate-ref',default='HEAD')
p.add_argument('--builder',default='twelve-all-builder')
p.add_argument('--image',default='twelve-all-bench')
p.add_argument('--volume',default='twelve-bench-data')
p.add_argument('--duration',type=int,default=15)
p.add_argument('--trials',type=int,default=3)
p.add_argument('--workloads',default='feed,post,mixed,create,like')
p.add_argument('--out',type=pathlib.Path,default=HERE/'optimization-results.json')
a=p.parse_args()
lock=(HERE/'.benchmark.lock').open('w')
try: fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
except BlockingIOError: raise SystemExit('Another comparison is active.')
def run(args,check=True):
 r=subprocess.run(args,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
 if check and r.returncode: raise RuntimeError(f'{args!r}: {r.stdout}')
 return r.stdout,r.returncode
def exec_builder(script): return run(['docker','exec',a.builder,'bash','-c','set -euo pipefail\n'+script])[0]
repo=pathlib.Path(run(['git','-C',str(HERE),'rev-parse','--show-toplevel'])[0].strip())
folder=HERE.parent.relative_to(repo)
prefix='twelve-revisions-'+uuid.uuid4().hex[:8]
root='/bench/'+prefix
names=['before','after'];workloads=a.workloads.split(',')
if not workloads or any(w not in ['feed','post','mixed','create','like'] for w in workloads): p.error('unknown workload')
if a.trials<1 or a.duration<1: p.error('duration and trials must be positive')
metadata={'base_ref':a.base_ref,'candidate_ref':a.candidate_ref,'source':{},'duration_s':a.duration,'warmup_s':2,'trials':a.trials,'connections':64,'server_cpu':'0','client_cpus':'1,2','server_memory_bytes':2*1024**3,'server_swap_bytes':0,'measured_at_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime())}
metadata['linux']=exec_builder('uname -a; lscpu')
metadata['compiler']=exec_builder('rustc --version; gcc --version | head -1')
metadata['wrk_sha256']=exec_builder('sha256sum /bench/wrk-mono/wrk')
rows=[];validations={}
def save():
 a.out.parent.mkdir(parents=True,exist_ok=True)
 a.out.write_text(json.dumps({'metadata':metadata,'validation':validations,'runs':rows},indent=2)+'\n')
with tempfile.TemporaryDirectory(prefix=prefix) as scratch:
 for name,ref in zip(names,[a.base_ref,a.candidate_ref]):
  sha=run(['git','-C',str(repo),'rev-parse',ref+'^{commit}'])[0].strip()
  blob=subprocess.check_output(['git','-C',str(repo),'archive',sha,str(folder)])
  dest=pathlib.Path(scratch)/name;dest.mkdir()
  with tarfile.open(fileobj=io.BytesIO(blob)) as archive:
   # Both revisions are limited to the submission directory by git archive.
   for entry in archive:
    parts=pathlib.PurePosixPath(entry.name).parts
    if len(parts)<=len(folder.parts): continue
    entry.name=str(pathlib.Path(*parts[len(folder.parts):]))
    archive.extract(entry,dest,filter='data')
  metadata['source'][name]={'commit':sha,'files':{str(f.relative_to(dest)):hashlib.sha256(f.read_bytes()).hexdigest() for f in dest.rglob('*') if f.is_file() and (f.suffix=='.rs' or f.name in ['Cargo.toml','Cargo.lock','build.sh','build.rs'])}}
  exec_builder('mkdir -p '+shlex.quote(root+'/'+name))
  run(['docker','cp',str(dest)+'/.',a.builder+':'+root+'/'+name])
  exec_builder('chown -R bench:bench '+shlex.quote(root+'/'+name))
  print('BUILD',name,sha,flush=True)
  run(['docker','exec','-u','bench','-e','CARGO_HOME=/home/bench/.cargo','-e','RUSTUP_HOME=/root/.rustup',a.builder,'taskset','-c','3','bash',root+'/'+name+'/build.sh'])
  metadata['source'][name]['binary_sha256']=exec_builder('sha256sum '+root+'/'+name+'/target/release/twelve-rust-poll').split()[0]
 for filename in ['mixed.lua','observe.lua','create.lua','like.lua']:
  run(['docker','cp',str(HERE/filename),a.builder+':'+root+'/'+filename])
 # All builds finish before any timed load. Each trial starts from the same seed.
 def start(name):
  container=prefix+'-server'
  run(['docker','run','-d','--name',container,'--network','host','--user','bench','--cpuset-cpus','0','--cpus','1','--memory','2g','--memory-swap','2g','--ulimit','nofile=65535:65535','-v',a.volume+':/bench','-e','SQLITE_PATH=/tmp/feed.db','-e','JWT_SECRET=twelve-dollar-challenge','-e','HOST=127.0.0.1','-e','PORT=3000',a.image,'bash','-c','cp /bench/seed/feed.db /tmp/feed.db; exec '+root+'/'+name+'/target/release/twelve-rust-poll'])
  for _ in range(600):
   _,code=run(['docker','exec',a.builder,'curl','--max-time','1','-sf','http://127.0.0.1:3000/health'],False)
   if code==0:return container
   time.sleep(.1)
  raise RuntimeError('server did not start')
 def stop():run(['docker','rm','-f',prefix+'-server'],False)
 def metrics(raw):
  socket=re.search(r'Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)',raw)
  def count(pattern):
   m=re.search(pattern,raw);return int(m[1]) if m else 0
  return {'rps':float(re.search(r'Requests/sec:\s+(\S+)',raw)[1]),'p95_us':count(r'p95_us=(\d+)'),'p99_us':count(r'p99_us=(\d+)'),'socket_errors':list(map(int,socket.groups())) if socket else [0,0,0,0],'non2xx':count(r'Non-2xx or 3xx responses: (\d+)'),'semantic_errors':count(r'HTTP semantic errors: (\d+)')}
 try:
  for name in names:
   start(name)
   raw,code=run(['docker','exec',a.builder,'bash','/bench/all/challenge/test/test.sh','http://127.0.0.1:3000'],False)
   validations[name]={'exit_code':code,'raw':raw}
   stop();save()
   if code:raise RuntimeError('official checks failed: '+name+'\n'+raw)
  for trial in range(a.trials):
   rotation=trial%len(workloads)
   for workload in workloads[rotation:]+workloads[:rotation]:
    order=names if (trial+workloads.index(workload))%2==0 else names[::-1]
    for name in order:
     container=start(name)
     script=root+'/'+(workload+'.lua' if workload in ['mixed','create','like'] else 'observe.lua')
     url='http://127.0.0.1:3000'+{'feed':'/feed','post':'/posts/500000','mixed':'','create':'','like':''}[workload]
     cmd=['docker','exec',a.builder,'taskset','-c','1,2','/bench/wrk-mono/wrk','-t2','-c64','--latency','-s',script]
     warm=run(cmd+['-d2s',url])[0];raw=run(cmd+['-d'+str(a.duration)+'s',url])[0]
     row=dict(variant=name,workload=workload,trial=trial,order=order,raw=raw,warmup_raw=warm,warmup_metrics=metrics(warm),**metrics(raw))
     row['server_log']=run(['docker','logs',container])[0]
     rows.append(row);stop();save()
     print('RUN',name,workload,trial,round(row['rps']),flush=True)
     if any(row['socket_errors']) or row['non2xx'] or row['semantic_errors'] or any(row['warmup_metrics']['socket_errors']) or row['warmup_metrics']['non2xx'] or row['warmup_metrics']['semantic_errors']:raise RuntimeError('load errors: '+str(row))
 finally:
  stop();save()
  exec_builder('rm -rf '+shlex.quote(root))
for workload in workloads:
 medians={n:statistics.median(r['rps'] for r in rows if r['variant']==n and r['workload']==workload) for n in names}
 print(workload,medians,'gain',round(100*(medians['after']/medians['before']-1),2),flush=True)
