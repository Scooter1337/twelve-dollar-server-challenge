# Mixed-workload optimization experiments

The submitted server and build profile remain at `b58c862`. These experiments did not establish a repeatable mixed-workload gain that meets the throughput and memory requirements.

All timed comparisons use the native Xeon E5-2690 v3 on MEGASERVER: one server CPU/quota, 2 GiB without swap, 65,535 hard/soft descriptors, 1,024 connections and separate client CPUs. Each trial starts a fresh process and database. Application affinity and host kernel settings are unchanged. The unchanged mixed generator uses live feed IDs, signature verification and real committed writes. These are saturation comparisons, not k6 capacity scores.

## Batching, SQL and SQLite compiler

Larger batches, recent-write adaptive scheduling, zero-delay polling, grouped like counts, revised SQLite PGO training and Clang 14 SQLite PGO were tested. None met the repeatable-gain requirement. An initial 128/zero-delay pilot had a median +8.2% over current Rust; the normal-build adaptive confirmation instead had -1.1% across eight balanced trials. The pilot is not a submitted improvement. Grouped counts added about 5.3% instructions and 7.2% cycles per completed request in separate diagnostic runs. Clang PGO's six-trial mixed median was 1.5% below control.

## Allocators and inlining

Fat LTO, `opt-level=3`, one codegen unit, native CPU instructions and abort-on-panic were already enabled. Rust-only compiler variants link the exact same SQLite archive, SHA-256 `18d5ff8f5e67bb8ba2f56e7b3fdd95a57f8f533a999c5b96d5d92b3244798aaf`. Every allocator experiment uses the identical control executable. Preload and allocator environment options are disposable harness settings; no extra runtime configuration or allocator dependency is added to the submission. Dynamic binding checks confirm that both Rust and SQLite calls use the alternate allocator.

The first screen uses four rotated 15-second measurements with five-second warmups. Its eight-variant order is exploratory rather than fully balanced. Changes below are ratios of independent medians against that stage's control:

| Variant | Mixed change |
|---|---:|
| jemalloc 5.3.0 | -8.8% |
| mimalloc 2.0.9 | -5.0% |
| glibc one arena | -2.8% |
| selective `inline(always)` | -5.8% |
| maximal application `inline(always)` | -9.7% |
| LLVM inline threshold 1,000 | -0.01% |
| thin LTO | -3.0% |

Both distribution allocators and maximal inlining lost all four pairs. Maximal inlining increases GNU `size` text from 1,429,136 to 1,592,524 bytes (+11.4%); threshold 1,000 increases it to 1,653,152 (+15.7%). This is a code-size observation, not proof that instruction-cache misses caused a slowdown.

The stronger inlining confirmation uses six fully balanced 30-second measurements with 15-second warmups:

| Variant | Median req/s | Change of medians | Median paired change | Paired range |
|---|---:|---:|---:|---:|
| Current | 48,063 | — | — | — |
| Selective hints | 44,118 | -8.2% | -5.4% | -14.7% to +6.8% |
| Threshold 1,000 | 48,868 | +1.7% | +0.4% | -10.0% to +20.8% |

The latest stable allocator releases were built from pinned source with `-O3 -march=native`: jemalloc 5.4.0 (`7a34f18502e7b222724097cdcd499b437d189acc`) and mimalloc 3.5.3 (`d4881d338125e1cb7c47ba4cfb398d6f7c0c8d45`). Six fully balanced 15-second measurements follow five-second warmups:

| Allocator | Median req/s | Change of medians | Median paired change | RSS MiB | Peak cgroup MiB |
|---|---:|---:|---:|---:|---:|
| Current glibc | 49,934 | — | — | 38.38 | 286.04 |
| jemalloc 5.4.0 | 50,466 | +1.1% | -2.4% | 40.05 | 287.04 |
| mimalloc 3.5.3 | 53,464 | +7.1% | +3.1% | 74.84 | 322.32 |

Mimalloc won five of six pairs but lost one by 8.1%; its independent-median gain is 7.0696% before rounding. RSS nearly doubles, with 40–42 MiB of anonymous huge pages versus zero for control. RSS and total container memory are different measurements. None of these allocator changes is submitted on this evidence.

The memory-policy comparison uses six fully balanced 20-second measurements with five-second warmups, comparing the current allocator, default mimalloc 3.5.3 and the same library with `MIMALLOC_ALLOW_THP=0`. This sets only the allocator's own-process THP policy; the host's kernel settings remain unchanged.

| Allocator | Median req/s | Change of medians | Median paired change | RSS MiB | Peak cgroup MiB |
|---|---:|---:|---:|---:|---:|
| Current glibc | 43,698 | — | — | 38.83 | 286.85 |
| Default mimalloc 3.5.3 | 44,159 | +1.1% | +2.6% | 73.92 | 321.53 |
| Mimalloc with THP disabled | 44,638 | +2.2% | +5.8% | 41.21 | 288.72 |

Disabling THP reduces the allocator's RSS by 44.3%, but it still exceeds current RSS by 6.1%. The tuned variant wins five pairs and loses one by 6.4%; its paired range is -6.4% to +13.1%. Default mimalloc's earlier +7.1% independent-median result falls to +1.1% in this new session. Neither result establishes the requested reliable mixed gain or no-throughput-loss behavior. No allocator, compiler, batching or SQL experiment is retained.

## Diagnostic interpretation

Hardware-counter runs are separate from throughput comparisons. They enclose the full load launch/run/teardown, count actual completed client requests, and record cycles, reference cycles, instructions, branches and branch misses. Per-request values include connection and shutdown work. Counter runs and heap instrumentation do not support throughput claims.

Two forward/reverse 20-second counter runs per variant use eight-second warmups; all five events run 100% of every interval. Medians:

| Variant | Instructions/request | Cycles/request | Branch misses/request |
|---|---:|---:|---:|
| Current glibc | 78,513 | 45,638 | 117.96 |
| Selective hints | 78,467 | 46,554 | 119.32 |
| Threshold 1,000 | 78,056 | 45,874 | 129.42 |
| Maximal hints | 78,783 | 46,532 | 122.92 |
| jemalloc 5.4.0 | 79,085 | 47,799 | 117.9 |
| mimalloc 3.5.3 | 78,295 | 45,350 | 115.58 |

The higher inline threshold saves 0.58% of instructions but increases branch misses per request by about 9.7%. Mimalloc saves 0.28% of instructions; jemalloc adds 0.73%. These observations do not identify a large allocator or inlining saving. They do not prove a specific cache-stall cause.

A separate glibc interposer counts executable heap calls around an instrumented 20-second mixed load: 1,471,209 malloc, 211,833 realloc and 1,471,209 free calls across 895,642 completed client requests. That is approximately 1.879 allocation/reallocation calls per completed request. Counts include SQLite, networking and connection setup/teardown, exclude hidden libc-internal calls, and can include work beyond the client completion window. This is distinct from the zero-allocation assertion for warmed Rust handlers. No whole-process zero-allocation claim is made.

CPU clocks vary substantially on this host. Ratios of independent medians, medians of paired ratios and individual trials are therefore shown separately. No frequency-normalized number is used as a measured speedup. The earlier unchanged-current/C comparison can reverse its apparent ranking between sessions without any source changes.

Every timed variant passes all 42 official checks before timing, and all completed warmups and measurements report zero socket, status and semantic errors. Builds and package installation finish before the associated timed loads. Runtime SQL, authentication, schema, durability and buffer ownership are preserved. Because no candidate satisfies the acceptance criteria, the existing source/capacity results remain applicable; no new k6 score is claimed.

## Evidence and cleanup

[`bench/mixed-tuning-results.json.gz`](bench/mixed-tuning-results.json.gz) retains the raw measurements, API checks, warmup errors, counter CSVs, memory snapshots, source/compiler/library/binary hashes, candidate sources and runner commands. Every downloaded archive member was checked against its SHA-256 and size before teardown. The owned remote directory, controllers, containers and images were removed; independent verification confirmed cleanup and that the unrelated database and Redis containers remained running. Cleanup logs are included. No prebuilt server or allocator binary is shipped.
