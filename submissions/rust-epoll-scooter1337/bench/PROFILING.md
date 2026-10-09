# CPU and memory profiling

The io_uring follow-up and differential SQLite profiles are documented in [IO_URING.md](../IO_URING.md). The measurements below describe an earlier epoll revision.

Measured on 7 October 2026 on the native Xeon E5-2690 v3 host. The application was revision `f631fff`, rebuilt with debug symbols, native instructions and the normal SQLite PGO training. Profiling is diagnostic; these runs are excluded from throughput comparisons and capacity scores.

Each workload used a fresh seed, 64 connections, one server CPU with a 2-GiB/no-swap limit, and two separate client CPUs. After warmup, `perf record -F 199 --call-graph dwarf,8192` sampled 20 seconds of a 25-second load. A separate load collected 20 seconds of hardware counters. No kernel settings were changed. CPU sets were test-harness resource isolation; the submitted server does not set affinity.

| Workload | Instructions/cycle | L1 load miss % | LLC load miss % | Branch miss % |
|---|---:|---:|---:|---:|
| Feed | 1.88 | 3.62 | 0.06 | 0.51 |
| Post | 1.21 | 6.93 | 0.02 | 1.22 |
| Mixed | 1.69 | 4.26 | 2.16 | 0.81 |
| Create | 1.12 | 7.05 | 7.01 | 1.66 |
| Like | 1.35 | 7.84 | 5.50 | 1.13 |

Counters were multiplexed, with events active for roughly 16–41% of the interval. Values are scaled estimates, not precise simultaneous measurements. Samples include kernel execution on behalf of the server.

Feed CPU samples concentrate in SQLite's VM (19.07%), table lookup (11.28%) and index lookup (9.09%); JSON escaping accounts for 4.09%. Mixed samples show the same SQLite paths plus mmap page lookup (8.90%). Small-concurrency read throughput therefore has substantial SQLite execution cost, with little evidence of LLC misses dominating feeds. Single-post samples are distributed across the kernel's socket/syscall/security paths. SHA-256 compression accounts for 4.39% of create samples and 8.65% of like samples, motivating a tested crypto-library replacement. Flat sample percentages are approximate and do not represent total inclusive call-path cost.

## Code-layout diagnostic

Rejected compact variants increased feed branch-miss rates from 0.532% to 1.191–1.716%, while instruction-fetch stall rates fell from 5.014% to 4.422–4.495%. IPC fell from 1.859 to 1.754–1.637. These are medians of two 20-second measurements, with counters active 57–71% of the interval. This supports investigating branch prediction rather than attributing the slowdown to instruction-cache misses. It does not identify a specific responsible branch. The rejected x86-64 candidate aligns Rust and SQLite functions to 64-byte boundaries; ARM64 retains default alignment because the same option regressed two ARM64 workloads.

## High-concurrency diagnostic

A separate run used unchanged `bench/load.js`, 190,000 VUs and a **two-minute** hold, with k6 profiling enabled. It is not a scored five-minute hold. The server retained its one-CPU/2-GiB limit; the generator used other physical cores and a 48-GiB/no-swap budget.

During that run, server IPC was 1.00, L1 load misses 6.93%, and LLC load misses 21.47%. This includes kernel activity and cache competition from other cores; it does not identify which application functions missed or prove that cache misses caused the observed tail latency.

k6 recorded 382.52 CPU-seconds over 30.06 seconds, approximately 12.7 cores. Its profile includes GC assist (25.24% cumulative), background marking (16.75%) and span scanning (32.52%). Those call paths overlap and must not be added. The live heap profile was 28,044.68 MB; creating JS VUs accounted for 60.69% cumulatively. Generator GC and memory pressure are material limits in this shared-host setup, but the profiles do not prove they are the only limits. Capacity requires a separate generator to isolate the server ceiling.

RSS, anonymous memory and total cgroup memory are distinct. SQLite mmap pages contribute to RSS; the cgroup also includes filesystem cache and socket/kernel memory. Disabling mmap reduced RSS in an experiment but increased total cgroup memory because a larger SQLite page cache was needed. That change was rejected, along with variants showing throughput regressions.

## Memory change

The rejected candidate descriptor table reserves fixed address space but initializes only the used prefix, in 1,024-slot blocks. It preserves the full connection layout and never moves active entries. In its three-trial mixed comparison, Xeon median RSS fell from 36.742 to 31.961 MiB and anonymous memory from 7.672 to 2.738 MiB. ARM64 RSS fell from 46.809 to 44.266 MiB and anonymous memory from 9.520 to 6.516 MiB. These are process measurements, not total droplet memory. Native seed copies were made outside the server cgroup, which can charge file-cache pages to the host; native cgroup totals must not be interpreted as whole-host physical usage or compared directly to Docker Desktop totals.

The longer five-pair mixed confirmation regressed 1.66% (27,364.93 to 26,909.97 req/s), so this memory reduction and function alignment were not submitted. Crypto replacement, dependency removal, smaller connection layouts and alternate SQLite cache settings were also tested and rejected for throughput regressions. That epoll revision retained 25 resolved packages including SQLite.

## Evidence and reproduction

`score-profile-results.json.gz` contains raw counter output, reports, folded stacks, k6 profile summaries, memory snapshots, source/binary identities, measured comparisons and runner source. Raw perf/pprof binaries and generated flamegraphs were retained locally, not shipped as application artifacts. FlameGraph revision: `41fee1f99f9276008b7cd112fca19dc3ea84ac32`.

Build a disposable copy with the normal `build.sh`, adding `CARGO_PROFILE_RELEASE_DEBUG=1`, `CARGO_PROFILE_RELEASE_STRIP=false`, and `CFLAGS=-g`. Run perf against the server PID during load:

```bash
perf record --no-buildid-cache -F 199 --call-graph dwarf,8192 -p "$SERVER_PID" -o record.data -- sleep 20
perf script -i record.data > record.script
stackcollapse-perf.pl record.script > workload.folded
flamegraph.pl workload.folded > workload.svg
perf stat -x, -p "$SERVER_PID" -e cycles,instructions,cache-references,cache-misses,branches,branch-misses,L1-dcache-loads,L1-dcache-load-misses,LLC-loads,LLC-load-misses,dTLB-loads,dTLB-load-misses -- sleep 20
```

Use the existing permissions for perf, rather than changing `perf_event_paranoid`. k6 exposes CPU/heap profiles when run with `--profiling-enabled --address 127.0.0.1:6566`; retrieve `/debug/pprof/profile?seconds=30` and `/debug/pprof/heap`, then inspect with `go tool pprof`. Keep profiling runs separate from scored measurements.
