#!/usr/bin/env python3
"""Run correctness checks and identical native ARM64 comparisons of all pinned submissions.
Use prepare instructions in README.md first. Does not change any application source.
"""
import argparse, fcntl, hashlib, json, os, pathlib, platform, random, re, statistics, subprocess, time, uuid

HERE = pathlib.Path(__file__).resolve().parent
p = argparse.ArgumentParser()
p.add_argument('--builder', default='twelve-all-builder')
p.add_argument('--image', default='twelve-all-bench')
p.add_argument('--volume', default='twelve-bench-data')
p.add_argument('--duration', type=int, default=15)
p.add_argument('--trials', type=int, default=3)
p.add_argument('--warmup', type=int, default=2)
p.add_argument('--connections', type=int, default=64)
p.add_argument('--variants', default='')
p.add_argument('--workloads', default='feed,post,mixed')
p.add_argument('--skip-validation', action='store_true')
p.add_argument('--validation-only', action='store_true')
p.add_argument('--out', type=pathlib.Path, default=HERE/'results.json')
a = p.parse_args()
lock_file=(HERE/'.benchmark.lock').open('w')
try:
    fcntl.flock(lock_file,fcntl.LOCK_EX|fcntl.LOCK_NB)
except BlockingIOError:
    raise SystemExit('Another benchmark is active. Stop it before starting a new run.')
lock_file.write(str(os.getpid())+'\n'); lock_file.flush()
manifest = json.loads((HERE/'manifest.json').read_text())
names = ['rust'] + [s['id'] for s in manifest['submissions']]
if a.variants:
    names = a.variants.split(',')
prefix = 'twelve-all-' + uuid.uuid4().hex[:8]
evidence = a.out.parent/'evidence'
evidence.mkdir(parents=True, exist_ok=True)

def capture(args, check=True, timeout=None):
    r = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=timeout)
    if check and r.returncode:
        raise RuntimeError(f'{args!r} failed ({r.returncode}): {r.stdout}')
    return r.stdout, r.returncode

def execute(script):
    return capture(['docker','exec',a.builder,'bash','-c',script])[0]

nginx = {'pr2', 'pr12'}
metadata = {
    'manifest': manifest, 'host_platform': platform.platform(), 'measured_at_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
    'linux': execute('uname -a; lscpu'), 'runtime_versions': execute('python3 --version; /opt/ruby/bin/ruby --yjit --version; /opt/erlang/29.1.1/bin/erl +S 1:1 +SDcpu 1:1 +SDio 1 +A 1 -noshell -eval \'io:format("~s~n", [erlang:system_info(system_version)]), halt().\'; bun --version; nginx -v; rustc --version; gcc --version | head -1; clang++ --version | head -1; sqlite3 --version'),
    'wrk_sha256': execute('sha256sum /bench/wrk-mono/wrk'),
    'seed_sha256': execute('sha256sum /bench/seed/feed.db /bench/seed/tokens.json'),
    'binary_sha256': execute("find /bench/all/source -type f \\( -path '*/bin/server' -o -path '*/target/release/twelve-rust-poll' \\) -print0 | sort -z | xargs -0 sha256sum"),
    'seed_content_sha256': '738d11ab3164c13177e347f75038d1e80b43c64e754544a42d1c32bd8d7bcf1d (verified before this batch)',
    'source_tree_sha256': execute("find /bench/all/source -type f \\( -name '*.rs' -o -name '*.ts' -o -name '*.py' -o -name '*.rb' -o -name '*.erl' -o -name 'server.c' -o -name 'server.cpp' -o -name 'main.cpp' -o -name 'Cargo.lock' -o -name 'Gemfile.lock' -o -name 'requirements.txt' \\) -not -path '*/.deps/*' -not -path '*/vendor/*' -not -path '*/.venv/*' -not -path '*/target/*' -print0 | sort -z | xargs -0 sha256sum"),
    'duration_s': a.duration, 'warmup_s': a.warmup, 'trials': a.trials, 'connections': a.connections,
    'workloads': a.workloads.split(','),
    'wrk_threads': 2, 'server_cpu': '0', 'client_cpus': '1,2', 'server_cpu_quota': 1,
    'server_memory_bytes': 2*1024**3, 'server_swap_bytes': 0,
    'load_generator': 'checksum-pinned wrk 4.2.0, only time_us changed to CLOCK_MONOTONIC for every server',
    'deployment': {n: ('documented Nginx + app, same cgroup' if n in nginx else 'direct HTTP') for n in names},
    'order_seed': 20261006,
    'image_identity': capture(['docker','image','inspect','--format','{{.Id}}',a.image])[0].strip(),
}
runs, validations = [], {}

def save():
    a.out.parent.mkdir(parents=True, exist_ok=True)
    a.out.write_text(json.dumps({'metadata':metadata,'validation':validations,'runs':runs}, indent=2)+'\n')

def start(name):
    container = prefix + '-server'
    args = ['docker','run','-d','--name',container,'--network','host','--cpuset-cpus','0','--cpus','1',
        '--memory','2g','--memory-swap','2g','--ulimit','nofile=65535:65535','-v',a.volume+':/bench',
        '-e','SQLITE_PATH=/tmp/feed.db','-e','JWT_SECRET=twelve-dollar-challenge','-e','HOST=127.0.0.1',
        '-e','PORT=3000',a.image,'bash','/bench/all/harness/launch.sh',name]
    capture(args)
    url = 'http://127.0.0.1:' + ('80' if name in nginx else '3000')
    try:
        deadline = time.monotonic()+60
        while time.monotonic()<deadline:
            out, code = capture(['docker','exec',a.builder,'curl','--max-time','2','-sf',url+'/health'],check=False)
            if code==0: return container, url, out
            state = capture(['docker','inspect','--format','{{.State.Running}}',container])[0].strip()
            if state != 'true': raise RuntimeError('server exited: '+capture(['docker','logs',container])[0])
            time.sleep(.1)
        raise RuntimeError('no healthy response within 60 seconds: '+capture(['docker','logs',container])[0])
    except Exception:
        capture(['docker','rm','-f',container],check=False)
        raise

def logs(container):
    out = capture(['docker','logs',container],check=False)[0]
    app = capture(['docker','exec',container,'bash','-c','test ! -f /tmp/app.log || cat /tmp/app.log'],check=False)[0]
    return out + app

def stop(container):
    capture(['docker','rm','-f',container],check=False)

def cpu(container):
    raw = capture(['docker','exec',container,'cat','/sys/fs/cgroup/cpu.stat'])[0]
    return int(re.search(r'usage_usec (\d+)',raw).group(1))

def metrics(raw):
    socket = re.search(r'Socket errors: connect (\d+), read (\d+), write (\d+), timeout (\d+)',raw)
    def count(pattern):
        m = re.search(pattern,raw); return int(m.group(1)) if m else 0
    return {'rps':float(re.search(r'Requests/sec:\s+(\S+)',raw).group(1)),
        'p95_us':float(re.search(r'p95_us=(\d+)',raw).group(1)), 'p99_us':float(re.search(r'p99_us=(\d+)',raw).group(1)),
        'socket_errors': list(map(int,socket.groups())) if socket else [0,0,0,0],
        'non2xx': count(r'Non-2xx or 3xx responses: (\d+)'), 'semantic_errors': count(r'HTTP semantic errors: (\d+)')}

def memory(container):
    # Capture cgroup peak before starting a Python process in the server cgroup.
    peak = int(capture(['docker','exec',container,'cat','/sys/fs/cgroup/memory.peak'])[0])/1024**2
    code = '''import pathlib, os, json, re
rows=[]
for path in pathlib.Path('/proc').glob('[0-9]*/status'):
    if int(path.parent.name)==os.getpid(): continue
    try: s=path.read_text()
    except OSError: continue
    if 'VmRSS:' not in s: continue
    def val(key):
        m=re.search(r'^'+key+r':\\s+(\\d+)',s,re.M)
        return int(m.group(1))/1024 if m else 0
    rows.append(dict(pid=int(path.parent.name),name=re.search(r'^Name:\\s+(.*)',s,re.M).group(1),rss_mib=val('VmRSS'),hwm_mib=val('VmHWM'),anon_mib=val('RssAnon')))
print(json.dumps(rows))
'''
    rows = json.loads(capture(['docker','exec',container,'python3','-c',code])[0])
    return {'processes':rows,'rss_sum_mib':sum(r['rss_mib'] for r in rows), 'anon_sum_mib':sum(r['anon_mib'] for r in rows),'cgroup_peak_mib':peak}

try:
    if not a.skip_validation:
        for name in names:
            container = None
            try:
                container, url, health = start(name)
                raw, code = capture(['docker','exec',a.builder,'bash','/bench/all/challenge/test/test.sh',url],check=False,timeout=180)
                count=re.search(r'passed=(\d+) failed=(\d+)',raw)
                validations[name]={'exit_code':code,'passed':None if count is None else int(count.group(1)),
                    'failed':None if count is None else int(count.group(2)), 'raw':raw,'health':health,'server_log':logs(container)}
                (evidence/(name+'-official-tests.log')).write_text(raw+'\nSERVER LOG\n'+logs(container))
                print('VALIDATED',name,code,raw.splitlines()[-1] if raw else '',flush=True)
            except Exception as e:
                validations[name]={'exit_code':-1,'error':str(e)}
                print('VALIDATION FAILED',name,str(e),flush=True)
            finally:
                if container: stop(container)
                save()
    else:
        prior=json.loads(a.out.read_text())
        validations.update(prior['validation'])
        runs.extend(prior['runs'])
    if a.validation_only:
        raise SystemExit(0)
    eligible=[n for n in names if validations.get(n,{}).get('exit_code')==0]
    # Counterbalance trial order with ascending, descending, and seeded-shuffled orders.
    rng=random.Random(metadata['order_seed'])
    shuffled=eligible.copy(); rng.shuffle(shuffled)
    orders=[eligible, list(reversed(eligible)), shuffled]
    for trial in range(a.trials):
        workloads=a.workloads.split(',')
        rotation=trial%len(workloads)
        workloads=workloads[rotation:]+workloads[:rotation]
        for workload in workloads:
            order=orders[trial%len(orders)]
            shift=['feed','post','mixed'].index(workload)*max(1,len(order)//3)
            order=order[shift:]+order[:shift]
            for name in order:
                if any(r['variant']==name and r['workload']==workload and r['trial']==trial for r in runs): continue
                container=None
                row={'variant':name,'workload':workload,'trial':trial,'order':order}
                try:
                    container,url,health=start(name)
                    script='/bench/all/harness/'+('mixed.lua' if workload=='mixed' else 'observe.lua')
                    endpoint={'feed':'/feed','post':'/posts/500000','mixed':''}[workload]
                    base=['docker','exec',a.builder,'taskset','-c','1,2','/bench/wrk-mono/wrk','-t2','-c'+str(a.connections),'--latency']
                    warm=capture(base+['-d'+str(a.warmup)+'s','-s',script,url+endpoint],timeout=a.warmup+60)[0]
                    before=cpu(container)
                    client_before=cpu(a.builder)
                    raw=capture(base+['-d'+str(a.duration)+'s','-s',script,url+endpoint],timeout=a.duration+60)[0]
                    client_after=cpu(a.builder)
                    after=cpu(container)
                    row.update(metrics(raw)); row.update(memory(container))
                    row.update(raw=raw,warmup_raw=warm,warmup_metrics=metrics(warm),cpu_usec=after-before,client_cpu_usec=client_after-client_before,health=health,server_log=logs(container))
                    print('RUN',name,workload,trial,round(row['rps']),row['socket_errors'],row['non2xx'],row['semantic_errors'],flush=True)
                except Exception as e:
                    row['error']=str(e)
                    if container: row['server_log']=logs(container)
                    print('RUN FAILED',name,workload,trial,str(e),flush=True)
                finally:
                    if container: stop(container)
                    runs.append(row); save()
    print('COMPLETE',len(runs),'runs',flush=True)
    for name in eligible:
        print(name,{w:statistics.median(r['rps'] for r in runs if r['variant']==name and r['workload']==w and 'rps' in r) for w in a.workloads.split(',')},flush=True)
finally:
    save()
