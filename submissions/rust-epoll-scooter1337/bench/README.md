# Reproduce the local comparison

The manifest pins 13 submission commits (#1–14) and the challenge snapshot. Newer submissions #15–18 are not included. The application sources and build flags are preserved. Preparation shares pinned Ruby and Erlang runtimes across submissions and selects the official checksum-pinned ARM64 Bun 1.4.2 asset in place of the installers' x86-64 asset.

Requires an ARM64 Docker host with four Docker CPUs, Python 3, git and network access. From this directory:

```bash
python3 prepare_environment.py
python3 compare_all.py
python3 compare_all.py --variants rust,pr5,pr6,pr9 --workloads mixed --out confirmatory-mixed.json
```

Preparation uses a temporary source/build directory; pass `--work /path/to/scratch` to select one. All application builds run as an unprivileged user. Every build finishes before timing begins. Each correctness check and trial uses a fresh copy of the official seed, whose complete row-content hash is verified.

The server stack is restricted to CPU 0, one CPU of quota, 2 GiB without swap and 65,535 file descriptors. The load generator uses CPUs 1 and 2. Rails #2 and #12 use the challenge's Nginx site configuration within the same server limits; other submissions receive HTTP directly. Nginx has one worker, 16,384 connections and a 75-second keep-alive timeout. Requests do not ask for compression.

Three trials per workload use two load threads, 64 connections, two seconds warmup and 15 seconds measurement. Trial order is ascending, descending and seeded shuffled, with workload rotations. Mixed traffic chooses relative weights of 100 feeds, 100 post reads, 15 likes and two creates, using live feed IDs and seed tokens. Writes are real. The follow-up repeats the four leading native implementations on mixed traffic.

`build_wrk.sh` pins wrk 4.2.0 and changes only its request/throughput clock to `CLOCK_MONOTONIC` for every implementation. An earlier probe observed backwards VM wall-clock steps that could underflow wrk's unsigned latency counter.

Results include raw measured/warmup output, official check results, binary/source hashes, runtime versions, CPU use and memory. Summed process RSS can double-count shared pages and includes the launcher. Peak cgroup memory includes seed copying, startup, page cache and kernel memory. The checked-in results are compressed JSON; inspect them with:

```bash
gzip -dc results.json.gz | python3 -m json.tool
gzip -dc confirmatory-mixed.json.gz | python3 -m json.tool
```

Each generated server container is removed after its run. Remove the idle builder and build-data cache when finished:

```bash
docker rm -f twelve-all-builder
docker volume rm twelve-bench-data
```

The comparison takes about 40 minutes after preparation, plus four minutes for the follow-up. Do not run simultaneous comparisons against the same host ports. `compare_all.py` uses a file lock to prevent overlap. Its `--builder`, `--image`, `--volume`, `--variants`, `--workloads` and `--out` options allow separate experiments.
