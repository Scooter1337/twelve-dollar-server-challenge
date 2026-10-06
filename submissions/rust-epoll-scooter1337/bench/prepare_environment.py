#!/usr/bin/env python3
"""Fetch pinned sources, install shared pinned runtimes, and build every submission.
Requires Docker, git, Python 3, network access, and a four-CPU Docker VM.
"""
import argparse, concurrent.futures, json, pathlib, shutil, subprocess, tempfile, threading, time

HERE=pathlib.Path(__file__).resolve().parent
p=argparse.ArgumentParser()
p.add_argument('--work',type=pathlib.Path)
p.add_argument('--rust-source',type=pathlib.Path,default=HERE.parent)
p.add_argument('--builder',default='twelve-all-builder')
p.add_argument('--image',default='twelve-all-bench')
p.add_argument('--volume',default='twelve-bench-data')
a=p.parse_args(); a.work=(a.work or pathlib.Path(tempfile.mkdtemp(prefix='twelve-server-compare-'))).resolve(); a.rust_source=a.rust_source.resolve()
manifest=json.loads((HERE/'manifest.json').read_text())
architecture=subprocess.check_output(['docker','info','--format','{{.Architecture}}'],text=True).strip()
if architecture not in ('aarch64','arm64'):
    raise SystemExit('This comparison harness installs native ARM64 runtimes. Use an ARM64 Docker host.')
a.work.mkdir(parents=True,exist_ok=True)
repo=a.work/'reference'; source=a.work/'source'; source.mkdir(exist_ok=True)
logs=HERE/'evidence'; logs.mkdir(exist_ok=True)

def cmd(args,**kw): return subprocess.run(args,check=True,**kw)
if not (repo/'.git').exists():
    cmd(['git','clone','https://github.com/arjaythedev/twelve-dollar-server-challenge.git',str(repo)])
cmd(['git','-C',str(repo),'fetch','origin','main','refs/pull/*/head:refs/remotes/origin/pr/*'])
for sha in [manifest['challenge']]+[s['sha'] for s in manifest['submissions']]:
    if subprocess.run(['git','-C',str(repo),'cat-file','-e',sha+'^{commit}'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode:
        cmd(['git','-C',str(repo),'fetch','origin',sha])

def archive(sha,path,dest,strip=2):
    if dest.exists(): shutil.rmtree(dest)
    dest.mkdir(parents=True)
    blob=subprocess.check_output(['git','-C',str(repo),'archive',sha]+([path] if path else []))
    cmd(['tar','-x',f'--strip-components={strip}','-C',str(dest)],input=blob)
for item in manifest['submissions']:
    archive(item['sha'],item['directory'],source/item['id'])
archive(manifest['challenge'],None,a.work/'challenge',strip=0)
shutil.copytree(a.rust_source,source/'rust',dirs_exist_ok=True,ignore=shutil.ignore_patterns('target','.deps','__pycache__'))
cmd(['docker','build','-t',a.image,'-f',str(HERE/'Dockerfile'),str(HERE)])
cmd(['docker','volume','create',a.volume],stdout=subprocess.DEVNULL)
cmd(['docker','run','-d','--name',a.builder,'--network','host','--ulimit','nofile=65535:65535',
    '-v',a.volume+':/bench','-v',str(source)+':/sources:ro','-v',str(a.work/'challenge')+':/challenge:ro',
    '-v',str(HERE)+':/harness:ro',a.image,'sleep','infinity'])
cmd(['docker','exec',a.builder,'bash','-c','''set -euo pipefail
mkdir -p /bench/all/source /bench/all/challenge /bench/all/harness /bench/seed
cp -a /sources/. /bench/all/source/
cp -a /challenge/test /challenge/bench /challenge/schema.sql /challenge/SPEC.md /bench/all/challenge/
cp /harness/launch.sh /harness/mixed.lua /harness/observe.lua /bench/all/harness/
chown -R bench:bench /bench/all/source
if [[ ! -f /bench/seed/feed.db ]]; then
  cp -a /challenge/seed /bench/all/challenge/
  (cd /bench/all/challenge; bash seed/make-seed.sh)
  cp /bench/all/challenge/seed/feed.db /bench/all/challenge/seed/tokens.json /bench/seed/
fi
sum=$(sqlite3 /bench/seed/feed.db 'SELECT * FROM users ORDER BY id; SELECT * FROM posts ORDER BY id; SELECT * FROM likes ORDER BY post_id, user_id;' | sha256sum | cut -d' ' -f1)
[[ $sum == 738d11ab3164c13177e347f75038d1e80b43c64e754544a42d1c32bd8d7bcf1d ]]
bash /harness/build_wrk.sh
'''])
state={}; lock=threading.Lock()

def run(name,script,user='root',cpus='3',timeout=2400):
    print('START',name,flush=True); started=time.time(); path=logs/(name+'.log')
    with path.open('w') as f:
        try:
            result=subprocess.run(['docker','exec','-u',user,a.builder,'taskset','-c',cpus,'bash','-c','set -euo pipefail\n'+script],
                                  stdout=f,stderr=subprocess.STDOUT,timeout=timeout)
            code=result.returncode
        except subprocess.TimeoutExpired: code=124
    with lock:
        state[name]={'exit_code':code,'elapsed_s':time.time()-started,'log':str(path.relative_to(HERE))}
        (HERE/'build-status.json').write_text(json.dumps(state,indent=2)+'\n')
    print('DONE',name,code,round(time.time()-started,1),flush=True)
    return code

ruby=(source/'pr12/install.sh').read_text(); ruby=ruby[ruby.index('work=$(mktemp -d)'):]
erl=(source/'pr11/install.sh').read_text(); erl=erl[erl.index('prefix=/opt/erlang/29.1.1'):]
with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
    ruby_job=pool.submit(run,'runtime-ruby-4.0.5',ruby,cpus='0,1')
    erl_job=pool.submit(run,'runtime-erlang-29.1.1','export BUILD_JOBS=1\n'+erl,cpus='2')
    run('runtime-bun-1.4.2-arm64','''tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 https://github.com/oven-sh/bun/releases/download/bun-v1.4.2/bun-linux-aarch64.zip -o "$tmp/bun.zip"
echo '54328bbc2d9c8e0c9f892c544d66c57a83b84139e34909e5ee81758f1ac8fda7  '"$tmp/bun.zip" | sha256sum -c -
unzip -q "$tmp/bun.zip" -d "$tmp"
install -m 755 "$tmp/bun-linux-aarch64/bun" /usr/local/bin/bun
bun --version
''')
    for n in [1,3,4,5,6,8,9,10,13]: run(f'build-pr{n}',f'bash /bench/all/source/pr{n}/build.sh',user='bench')
    run('build-rust','''export PATH=/root/.cargo/bin:$PATH
export RUSTUP_HOME=/root/.rustup CARGO_HOME=/home/bench/.cargo
bash /bench/all/source/rust/build.sh
''',user='bench')
    if ruby_job.result()==0:
        for n in [2,12,14]: run(f'build-pr{n}',f'bash /bench/all/source/pr{n}/build.sh',user='bench')
    if erl_job.result()==0: run('build-pr11','bash /bench/all/source/pr11/build.sh',user='bench')
cmd(['docker','commit',a.builder,a.image])
print('Ready: python3 compare_all.py',flush=True)
