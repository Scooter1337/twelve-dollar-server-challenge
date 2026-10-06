# Reproduce the local comparison

The manifest pins 13 submission commits (#1–14) and the challenge snapshot. Newer submissions #15–18 are not included. The application sources and build flags are preserved. Preparation shares pinned Ruby and Erlang runtimes across submissions and selects the official checksum-pinned ARM64 Bun 1.4.2 asset in place of the installers' x86-64 asset.

Requires an ARM64 Docker host with four Docker CPUs, Python 3, git and network access. From this directory:

The historical all-submission table uses Rust revision `7b8c351`. Use that revision's source with `--rust-source` to reproduce it; preparation defaults to the current source.

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

## Compare Rust revisions

After preparing the builder/image, this script archives both commits, builds them as `bench`, then uses fresh seeds with the same one-CPU/2-GiB limits and monotonic wrk. Builds finish before measurements. It requires Python 3.12+ and a Git clone containing both commits:

```bash
python3 compare_revisions.py --base-ref fbdbe2b --candidate-ref 2f6000c \
  --workloads mixed,create,like
```

Omit `--workloads` to include feed and single-post reads. The default is three 15-second trials with two-second warmups and 64 connections. Execution order alternates and workload order rotates. The creates workload sends real ASCII posts; likes target seed post 500000 with random seed users, so duplicate likes become common after warmup. Mixed is the same live-ID workload used above. Results contain raw output, official validation, source and binary hashes.

[`optimization-results.json.gz`](optimization-results.json.gz) contains the 30-run allocation experiment, four-run insert probe and 18-run final confirmation, plus allocation and protocol/recovery/connection validation. The local runner source and source hashes are included. Its order is recorded per trial; the revision script provides the same workloads/resource limits with alternating order.

## Official k6 workload

The local comparison also runs the challenge root's `bench/load.js` unchanged. Use checksum-verified k6 2.3.0 and a fresh seed for each revision. Run the golden API checks before the warmup; their expected rows no longer match once load-test writes have changed the database. Keep the same server and database running between warmup and hold.

From the challenge root, with the server already running at the URL shown:

```bash
k6 run --quiet --summary-export warmup.json \
  -e BASE_URL=http://127.0.0.1:3000 -e VUS=1000 -e DURATION=2m \
  -e TOKENS="$(pwd)/seed/tokens.json" bench/load.js
k6 run --quiet --summary-export hold.json \
  -e BASE_URL=http://127.0.0.1:3000 -e VUS=2500 -e DURATION=5m \
  -e TOKENS="$(pwd)/seed/tokens.json" bench/load.js
```

The recorded comparison runs server containers with one CPU and 2 GiB without swap, client containers on two separate CPUs with 2,800 MiB without swap, and 65,535 file descriptors. It adds `--local-ips 127.0.0.2,127.0.0.3,127.0.0.4,127.0.0.5` for this shared-host loopback setup. Builds finish first; both warmup and hold retain the script's default 60-second ramp-up and 30-second ramp-down. Full-run k6 summaries include those ramps and graceful stopping.

[`official-k6-results.json.gz`](official-k6-results.json.gz) contains both revisions' summaries, logs, source/script/binary hashes, sampled resource counters and the exact preparation/runner source. That revision comparison tests a fixed 2,500-user count; it does not establish the official droplet score.

## Capacity search

[`official-k6-capacity-results.json.gz`](official-k6-capacity-results.json.gz) records a later search on unchanged application revision `230b2f8`. It includes the valid M1 lower-bound runs, invalid transport probes, and the completed native x86-64 search. See [BENCHMARKS.md](../BENCHMARKS.md) for the hardware distinction, all probes and the 68,750/69,000-user boundary.

Use the same official script and thresholds. After fresh-seed API validation and the 1,000-user/two-minute warmup, retain the server/database, probe a starting count, and double until a valid server failure. Bisect the passing/failing bracket in 250-user steps, giving every probe a five-minute hold. Repeat the highest pass for another full hold; step down by 250 if confirmation fails. Abort on generator, transport or host resource failures instead of treating them as server capacity.

The archived native `prepare.sh` and `run.py` contain the exact commands, unprivileged build, resource limits, CPU assignments, eight `--local-ips` addresses and infrastructure checks. Adapt the host paths and available CPU numbers when reproducing. Native tests use one server CPU and 2 GiB without swap; the generator excludes that CPU and its SMT sibling. No builds run during timing. The official separate-client droplet test is still required for an official score.
