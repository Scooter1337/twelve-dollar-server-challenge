# io_uring follow-up

These measurements describe revision `b58c862`, before the subsequent read-transaction and fdatasync changes described in [README.md](README.md).

The submitted server uses Linux io_uring directly. Accepted sockets go into each ring's registered file table, rather than the process file table. This is the same Linux mechanism used by [C #26](https://github.com/arjaythedev/twelve-dollar-server-challenge/pull/26); it is not a replacement for Linux descriptors. Registration is bounded by the inherited hard limit for each ring, while occupied slots do not consume ordinary process descriptors.

With the repository's soft/hard limit of 65,535, Rust creates eight rings of 65,535 slots. With a 1,048,576 limit it creates one ring of 524,280 slots. The latter reduces worker coordination and ring resources while retaining the same aggregate table capacity. These are table capacities, not a promise that all those users fit in the CPU/memory budget.

Every worker holds one shared mutex for the complete SQLite/query/authentication/commit/checkpoint batch. The database remains a single in-process SQLite connection, with WAL and synchronous=NORMAL. Successful responses are queued only after commit. No request data or verified tokens are cached. The dense-range feed path discovers current IDs on every request, reads current rows and likes, and restores timestamp/ID order; sparse ranges use the original query. Both paths use the fixed schema and indexes.

Raw results, runner source, identities, validation logs and folded profiles are in [bench/io-uring-results.json.gz](bench/io-uring-results.json.gz). Build products and binary perf captures are excluded from the submission.

Connection records occupy 64 bytes. Partial requests allocate on demand. A submitted send owns a separate allocation until its final completion; idle connections retain no response buffer. Each ring keeps at most 256 reusable response allocations of at most 16 KiB. Generations protect reused slots from stale receive/send completions and pending replies. Backpressure pauses reads while output drains. Idle keep-alives remain open; partial inputs or blocked sends are shut down after 120 seconds.

## Throughput

The final published-Rust comparison uses four 20-second trials after ten-second warmups, 64 keep-alive connections, one server CPU, 2 GiB/no swap and a supplemental 1,048,576 soft/hard descriptor limit. For each workload the variant order is balanced: AB, BA, BA, AB (or its inverse). Every trial records zero socket, status and semantic errors. Source hashes match the selected source; read batching and fdatasync are absent.

| Workload | Published Rust median req/s (min–max) | Selected Rust median req/s (min–max) | Median change |
|---|---:|---:|---:|
| Feed | 28,498 (28,153–28,689) | 33,616 (33,272–33,712) | +18.0% |
| Single post | 107,886 (106,492–109,607) | 121,619 (120,966–122,004) | +12.7% |
| Mixed | 30,213 (29,337–31,153) | 41,500 (40,971–46,653) | +37.4% |
| Create | 9,802 (9,652–11,714) | 10,826 (10,053–12,384) | +10.4% |
| Like | 81,180 (73,725–82,310) | 93,101 (91,067–93,649) | +14.7% |

Feed, single-post, mixed and like throughput improve in every paired trial. Creation is less stable: paired changes range from −14.2% to +27.8%, so its higher median does not establish a repeatable gain in every run. Sampled clocks also vary during mixed and creation workloads.

The C #26 comparison uses four balanced 20-second trials, ten-second warmups, 1,024 connections and the repository's 65,535 soft/hard limit. This supplies enough active work for C's 128-completion / four-millisecond batching target; its 64-connection results can otherwise be constrained by that wait.

| Workload | Selected Rust median req/s (min–max) | Unmodified C #26 median req/s (min–max) | Rust median change |
|---|---:|---:|---:|
| Feed | 31,609 (31,075–32,174) | 30,201 (30,064–30,330) | +4.7% |
| Mixed | 47,380 (43,517–56,800) | 50,984 (47,782–54,287) | −7.1% |

Rust wins each feed pair. Mixed paired changes range from −17.6% to +18.9%; Rust's higher final trial does not overturn the lower balanced median. These are local throughput comparisons, not an official droplet score or a claim about every submission.

Earlier screening used an ordering bug: reversing odd trials sometimes canceled the intended rotation. Those raw rows remain in the evidence, but their percentages are provisional and do not support final performance claims. Correctness checks are unaffected.

### Issue #7's preference index

[The issue's U5/Z1 comparison](https://github.com/arjaythedev/twelve-dollar-server-challenge/issues/7#issuecomment-5995788219) weights mixed throughput 40%, application LOC 20%, resolved dependencies 20% and process RSS 20%. It is separate from the challenge's user-capacity score. Applying its fixed formula to the matched local published/current observations above gives 49.23 / 54.04, a 4.80-point increase. This is a native-Xeon sensitivity check with 1M descriptors, not an update to the issue's ARM64 ranking or a comparison to its reported 75.4 points.

The inputs are 30,213 / 41,500 mixed req/s, 1,329 / 2,373 physical application-source lines, 25 / 27 Linux normal dependencies including standalone SQLite, and 36.28 / 35.59 MiB median process RSS. The new implementation adds `io-uring` and `bitflags`; source size increases as the epoll comparison path is retained. The formula clips current LOC utility to zero and both RSS utilities to 100. Thus this local index gain comes from throughput, while high-connection memory savings benefit capacity rather than its already-clipped RSS component. Raw graphs and unrounded components are retained as `score-sensitivity.json` in the evidence.

## Validation

The exact current source passes the 42 official checks, 84 additional checks with 80,000 simultaneous sockets under the 65,535 server limit, and six reference-SQLite feed checks. The extra checks include 66-second reuse of original sockets, pipelining/backpressure, repeated reset/reuse, failed commits, per-request authentication, and SIGKILL recovery of acknowledged writes. The one-ring path passes 83 extra checks with the idle wait omitted, plus the feed checks. AddressSanitizer passes the same 83-check protocol/recovery suite and six feed checks in a separate diagnostic build.

The ARM64 install/build runs on Ubuntu 24.04 as an unprivileged user and passes the 42 official checks, 83 extra checks and six feed checks. ARM64 correctness results are not x86-64 performance measurements.

The allocation assertion counts zero Rust allocation/reallocation calls across 100 warmed ordinary requests on each of the five handlers. SQLite's C allocator, kernel I/O, cold connections, partial input, escaped values and buffer growth are outside that claim.

## Capacity

The unchanged repository `bench/load.js` passes a five-minute hold at 100,000 users for both selected Rust and unmodified C #26, after the specified 1,000-user/two-minute warmup. Both servers use one CPU, 2 GiB/no swap and the supplemental one-million descriptor limit. The 60-second ramp and ramp-down are included in k6's aggregate latency/error metrics. Neither client is OOM-killed, and both servers remain running.

| Variant | k6 p95 / p99 ms | Failed requests | Server cgroup peak MiB |
|---|---:|---:|---:|
| Selected Rust | 12.673 / 25.760 | 0 / 3,735,050 | 450.74 |
| Unmodified C #26 | 20.306 / 36.215 | 0 / 3,734,867 | 497.51 |

Rust's total server peak is 9.4% lower in this run. The k6 clients peak at approximately 24 GiB each, outside the server cgroup. Aggregate k6 rates are approximately 9,330 req/s and include ramp-down; user think times govern the offered load, so they are not maximum-throughput results. The load script SHA-256 is `14fbbe36a7bef2cc803c8c2309b8b93d6e6ec9c2b6aba78ebe3b9a95ebf85972`, matching the repository file.

A two-minute diagnostic hold at 300,000 users uses the one-million inherited descriptor limit, fresh database per variant, a 60-second ramp and 30-second ramp-down. All three variants reach 300,000 distinct connected users, with zero request/connect errors or timeouts. This is the attributed native generator, not unchanged k6 or the organizer's official score.

| Variant | Hold req/s | Hold p95 / p99 ms | Server cgroup peak MiB |
|---|---:|---:|---:|
| Selected Rust | 31,695 | 47.97 / 54.06 | 779.98 |
| Unmodified C #26 | 31,718 | 30.97 / 40.91 | 837.79 |

Throughput here is governed by user think time: these equal req/s do not establish equal maximum throughput. Hold-window median cgroup anonymous memory is 27.7 MiB for Rust and 70.9 MiB for C. Kernel memory is approximately 466–468 MiB and database file pages approximately 262 MiB in each case. The server's native connection storage is now much smaller than kernel connection storage. Rust's server peak is 6.9% lower at this load. These local savings are not a guarantee of the same saving on the droplet. The rejected immediate-read candidate's corresponding raw screen is retained separately.

The 400,000-user search probe initially sustains approximately 42,000 req/s, then accumulates socket backlog and is OOM-killed before completing its hold. Anonymous memory remains near 35 MiB; socket-buffer accounting grows from 17 MiB at 131 seconds to approximately 1,352 MiB at 152 seconds. Database file pages fall from 272 to 38 MiB as memory is reclaimed. The 393,750-user probe instead remains running but fails with 23.15% aggregate errors and roughly 60-second p95/p99 latency; hold CPU usage averages 96.3% of one core with only 0.017 seconds of quota throttling. These failed runs identify backlog amplification and memory pressure, rather than Rust heap growth, at the local boundary. They do not isolate the initiating stall or establish cache misses as its cause.

The two-minute binary search reaches 388,000 passing / 388,250 failing users, within 250. A subsequent five-minute confirmation at 388,000 fails with 6.747% aggregate errors, 35.5-second p95 and 60.2-second p99; its server remains alive and peaks at approximately 2 GiB. A 250-user step-down to 387,750 also fails, with 6.763% aggregate errors and a 60.2-second p99. Those earlier short-run passes are therefore not confirmed capacity claims. Raw passing, failed and incomplete-on-OOM probes are retained in the evidence.

Re-searching with five-minute holds narrows the supplemental one-million-descriptor boundary to 365,500 passing / 365,750 failing users. The passing count then passes a separate five-minute confirmation after the 1,000-user/two-minute warmup. C #26 also passes the same confirmed load. Neither client is OOM-killed, both servers remain running, and every user receives a successful live feed response.

| Variant at 365,500 users / 1M descriptors | Hold req/s | Hold p95 / p99 ms | Failed / total requests | Server cgroup peak MiB |
|---|---:|---:|---:|---:|
| Selected Rust | 38,586 | 49.43 / 62.76 | 0 / 13,504,482 | 981.44 |
| Unmodified C #26 | 38,633 | 43.00 / 59.12 | 0 / 13,518,294 | 1,094.37 |

Rust's peak is 10.3% lower in this pair; median anonymous memory is 31.94 versus 107.80 MiB. Median file pages are approximately 289 MiB and kernel memory 565–567 MiB. Both rates follow user think times and neither run establishes the C server's maximum capacity. The 365,750-user Rust failure has 2.362% aggregate errors and a 53.9-second aggregate p99. Its anonymous memory remains near 32 MiB while socket buffers grow above 1.2 GiB late in the hold. This is a local sustained boundary with observed stall sensitivity, not a guarantee that 250 users separate passing and failing loads on another machine.

Selected Rust also passes 365,500 users at the repository's 65,535 soft/hard descriptor limit, using eight rings, after the same warmup and five-minute hold. All 13,519,387 requests succeed, with zero connection failures or timeouts. Hold rate is 38,634 req/s, hold p95/p99 41.74/60.92 ms, aggregate p95/p99 40.91/60.92 ms, and server cgroup peak 1,009.52 MiB. Median anonymous memory rises to 48.15 MiB; the additional rings cost memory and coordination. Its 81.6% one-core CPU utilization is above the one-ring confirmation's 77.1%, with negligible quota throttling in both. This confirms the same load under the submitted descriptor limit; only the supplemental one-million limit was binary-searched, and neither is an official k6 capacity score.

## Branchless experiments

The end-to-end screening tables in this section predate the balanced-order correction. They are retained as rejected experiments, and small differences are not treated as causal gains or regressions.

Measured on the native Xeon E5-2690 v3 host, one server CPU, 2 GiB/no swap, 65,535 soft/hard descriptors, separate load CPUs, 64 connections, two-second warmup and three 20-second trials. Every variant links the identical SQLite archive, SHA-256 `18d5ff8f5e67bb8ba2f56e7b3fdd95a57f8f533a999c5b96d5d92b3244798aaf`, to avoid independently retrained PGO confounding the comparison. All variants pass 42 official checks and six feed checks.

| Variant | Feed median req/s (min–max) | Mixed median req/s (min–max) |
|---|---:|---:|
| Control | 32,181 (32,129–33,155) | 35,133 (34,674–41,222) |
| Sentinel like-count bucket, safe indexing | 31,692 (31,332–32,304) | 42,570 (34,406–44,039) |
| Sentinel plus unchecked lookup/count indexing | 31,983 (31,956–32,390) | 36,226 (34,452–38,733) |
| Bitwise JSON escape predicate | 32,217 (32,006–32,441) | 34,111 (33,655–38,849) |

The sentinel removes the branch that discards like rows outside the twenty current feed IDs. The unchecked variant additionally replaces two bounds checks with a documented range invariant. Feed medians were 1.52% and 0.61% below control. The mixed spread is substantial, so it does not establish a reliable gain. Neither variant is shipped under the no-throughput-regression requirement.

The JSON predicate replaces short-circuit Boolean OR with bitwise OR. Its release executable is byte-for-byte identical to control, SHA-256 `55112a3298c1f066935b0940e7db74fb9db0747fdf4700a623bafc3d4d04f4b1`. The compiler already optimized the condition; differences between those rows are measurement variation, not an optimization effect.

Separate twelve-second perf-stat intervals collect four nonmultiplexed events. Feed branch misses remain approximately 0.46% for control and both sentinel variants. Mixed branch misses were 1.037%, 1.124% and 0.889%, respectively. A lower branch-miss ratio alone does not establish higher throughput. No branchless or unchecked-index change was added to production.

An initial branchless batch linked a different retrained SQLite archive and is excluded. Raw results, compiler/source/binary identities and runner source are retained with the final benchmark evidence.

## Profile-guided experiments

The pre-read-batching io_uring source is profiled in a separate build with symbols and frame pointers, linked to the exact measured SQLite archive. Twenty-second `perf record -F 199 --call-graph dwarf,8192` intervals use fresh fixtures, warmup, 64 connections and the same one-CPU / 2-GiB limit. These diagnostic runs are excluded from throughput comparisons.

At the spec descriptor limit, feed flat samples put 24.36% in SQLite's interpreter, 8.37% in table lookup and 3.87% in JSON escaping. Folded inclusive stacks put approximately 60.15% under `sqlite3_step`, 12.18% under `unixFileSize`, and 4.20% under JSON escaping. Inclusive call paths overlap and must not be added. Mixed flat samples put 19.05% in the interpreter, 6.94% in table lookup and 2.65% in JSON escaping. SQLite transaction setup therefore offers a larger target than a JSON-only assembly rewrite.

Execution counters run without multiplexing: feed IPC 1.90 with 0.42% branch misses; mixed IPC 1.71 with 0.84% branch misses. Data-cache counters remain multiplexed at about 50–75% despite using four events per interval, so their scaled ratios are estimates: feed L1 load misses 3.95%, LLC load misses 0.09%; mixed L1 4.09%, LLC 1.77%. These low-concurrency observations do not identify the high-connection bottleneck or establish that cache misses dominate it.

A separate diagnostic replay at the confirmed 365,500 users and one-million descriptor limit uses the same selected source and SQLite archive, with symbols and frame pointers. It ramps for 60 seconds and holds for 75 seconds; it has zero errors and is excluded from scored capacity and throughput. A 20-second 199 Hz cycle capture retains 3,036 samples; a separate ten-second 99 Hz LLC-miss capture retains 754. Both report zero lost samples. The first decode while the container was present left user-space symbols unresolved. Re-decoding the retained captures after teardown against matching executable/library build IDs restores SQLite and Rust call stacks; both decodes and raw captures are retained locally.

Separate ten-second counter intervals run every event 100% of the time: L1 load misses 5.69%, LLC load misses 17.91%, IPC 1.37 and branch misses 1.15%. The LLC percentage is misses among LLC loads, not among all memory accesses or the percentage of time stalled. These are different load intervals and workloads from the low-concurrency profiles above, not a controlled cache regression comparison.

Period-weighted inclusive cycle stacks place 33.26% under `sqlite3_step`, 34.49% under `tcp_sendmsg`, 14.71% under SQLite B-tree work and 4.21% under JSON escaping. Commit-phase work is 4.06%; these overlapping call paths must not be added. The LLC-miss capture places 44.03% under TCP send paths, 13.60% under SQLite execution and 0.15% under JSON escaping. Kernel established-connection lookup is the largest flat LLC-miss symbol at 9.06%. The send paths include same-host loopback delivery and client wakeups, so this is not a separate-client network profile. Approximately 0.95% of cycle weight remains unknown.

SQLite execution and kernel network work are substantial targets at this load; JSON-only assembly remains a small one. Cache activity increases the cost of handling this working set, but miss ratios and sampled miss attribution do not quantify memory-stalled cycles. This stable replay also does not capture the initiating stall in the late five-minute failures. The supported failure mechanism remains socket-backlog amplification and file-page reclaim, with little anonymous growth; the exact trigger is unresolved.

The assembly experiment compares the existing 16-byte SIMD scanner, 32-byte AVX2 intrinsics and handwritten AVX2 instructions. Differential byte/escape tests, every vector tail through 512 bytes, and slices adjacent to inaccessible guard pages all pass. All server variants also pass the 42 official checks, 83 extra checks and six feed checks. Wider scanning improves long plain-string microbenchmarks but adds overhead on short fields; microbenchmark wins alone are not server throughput wins.

| Scanner | 12-byte string ns/call | 96-byte string ns/call | 384-byte string ns/call | Feed median req/s | Mixed median req/s |
|---|---:|---:|---:|---:|---:|
| Existing SIMD | 22.895 | 16.083 | 50.995 | 33,029 | 40,010 |
| AVX2 intrinsics | 33.054 | 13.355 | 28.404 | 32,113 | 39,633 |
| Handwritten AVX2 | 36.481 | 13.713 | 28.659 | 32,667 | 44,920 |

Microbenchmarks use nine rotated trials with one million calls per length/variant, warmed reusable output buffers and optimizer barriers. Server measurements use three 15-second trials, 64 connections and the one-million descriptor limit. Handwritten assembly is not faster than the equivalent AVX2 intrinsics on these long strings. Feed medians regress 2.77% and 1.10%; neither scanner is shipped. Mixed results vary substantially, including a control trial above either assembly trial, so the assembly mixed median alone is not proof of a gain.

A deferred-read transaction candidate adds a prepared `BEGIN`, holding one SQLite snapshot through the completion batch and upgrading it on writes. Its initial three 15-second medians improve feed 20.0% and single-post reads 27.8%, while mixed falls 5.7%. Five longer 45-second paired mixed trials do not repeat that result: the median paired change is +0.13%, with individual ratios from -12.79% to +2.99%. The ratio of the independent medians is -1.09%; paired and independent-median comparisons are different statistics. These observations do not establish either a reliable mixed gain or a repeatable 5.7% regression.

Separate diagnostic profiles link the same SQLite archive and collect nonmultiplexed execution counters. Approximate mixed instructions per request are 94,629 for control, 93,316 for deferred reads and 93,210 for immediate reads; cycles per request are 55,536, 55,702 and 55,946. Branch misses remain 0.83–0.89%. Event-running clock rates differ substantially: 2.50, 2.92 and 2.47 GHz. These intervals use the full load's average req/s to normalize a shorter profiler window, so per-request values are approximate. Clock-normalized values explain mechanisms; they never replace raw measured throughput.

Feed counters show a larger improvement: about 173,346 instructions / 90,800 cycles per request for control, against 157,594 / 75,411 for deferred and 158,886 / 75,488 for immediate reads. Folded cycle-period-weighted inclusive samples put file-stat syscall work at 8.37% in control and approximately 0.21–0.23% with read transactions. Mixed already holds transactions after writes: its file-stat work is almost absent in every variant. With the exact container libraries restored for stack unwinding, mixed inclusive SQLite commit work is 4.66% / 4.83% / 4.78% for control / deferred / immediate, while all work beneath sqlite3_step is about 48% and beneath tcp_sendmsg about 23%. Commit profiles remain similar; inclusive call paths overlap. These profiles support a feed transaction saving. They do not identify a large mixed transaction-upgrade or layout penalty; this differential execution-counter set does not measure data-cache misses.

Separate syscall tracing is diagnostic only and substantially reduces load. The three mixed variants perform approximately 0.51–0.53 fsync calls and 0.34–0.35 file-stat calls per thousand observed requests. They do not show extra synchronization work attributable to deferred upgrades. Broader write/checkpoint histories and CPU clock changes still limit small throughput claims.

The layout/mode ablation uses three 30-second mixed trials, each after a 30-second warmup. It tests the original deferred candidate, a single deferred `BEGIN` with no extra field, immediate transactions with an unused extra statement retained, and immediate transactions restricted to feed reads. All variants pass 42 official checks and link the identical SQLite archive.

The control median is 35,889 req/s; the four ablations and immediate candidate range from 34,382 to 35,247. These medians cannot by themselves isolate the cause: the control's first trial runs at a mean sampled 3.08 GHz while later trials and candidates commonly run at about 2.3–2.6 GHz. No variant shows a consistent advantage from removing read-to-write upgrades, removing the extra statement, or restricting batching to feeds. The supported conclusion is that batching targets read-only pager setup, which is already largely amortized on mixed writes; the large earlier mixed differences were not reproducible under paired comparisons. Clock and storage variation prevent a claim of zero mixed throughput loss from these experiments alone.

The selected source retains the pre-read-transaction behavior. Deferred and immediate read batching are experiments, not submitted optimizations: the read-only gain is clear, but mixed throughput does not consistently meet the no-regression requirement. The live dense-range feed scans and compact io_uring storage are retained.

A subsequent mixed diagnostic records cycles, reference cycles and instructions with all three events running 100% of their intervals. The same Rust binary runs at approximately 3.05 and 2.38 GHz of unhalted clock; C runs at 2.39 and 2.43 GHz. This independently confirms substantial clock variation. These profiled rows are excluded from final throughput results. SHA-256 uses its software backend on this Haswell CPU; the library code does not support attributing the clock difference to a SHA-256 AVX2 backend. Clock variation explains part of the spread without establishing a fixed clock disadvantage for either implementation.

Further experiments link the identical SQLite archive. A 3-trial screen at 1,024 connections compares the original 500-page cache / 1-GiB mmap limit with 8-MiB and 32-MiB caches and mmap disabled. Feed/mixed medians change −2.47%/−3.66% and +0.41%/+4.42%, respectively. Keeping mmap with the 32-MiB cache gives −2.52% feed / +3.97% mixed in a subsequent two-trial screen. The small mixed differences vary between trials, and the larger caches add memory. None is retained.

A per-request snapshot experiment shares one transaction across the three feed scans, ending it before the next request. It reuses `BEGIN IMMEDIATE`, skips the wrapper inside an existing write batch, and changes neither schema nor commit-before-reply behavior. Four balanced 20-second trials at 64 connections / 1M descriptors give:

| Workload | Control median req/s | Per-request immediate snapshot | Change |
|---|---:|---:|---:|
| Feed | 33,352 | 35,857 | +7.51% |
| Single post | 123,253 | 123,417 | +0.13% |
| Mixed | 43,310 | 39,504 | −8.79% |
| Create | 11,004 | 10,371 | −5.75% |
| Like | 93,686 | 92,083 | −1.71% |

Separate complete-load counter intervals count the actual completed requests, avoiding the earlier shorter-window normalization. Control/candidate mixed instructions per request are 93,585/93,054 and 93,533/93,572 in the two pairs. Cycles per request are 54,950/53,887 and 55,329/54,127. Every event runs 100% of its interval. Unhalted clock is 2.73/2.57 and 2.49/2.50 GHz. The candidate removes little mixed instruction work; these diagnostic rows do not replace the lower unprofiled mixed median.

Separate syscall tracing measures 3.001 versus 1.000 file-stat calls per read-only feed request. Mixed traffic measures only 0.337/0.341 file-stat calls and 0.506/0.512 fsync calls per thousand requests. Its pager/file-stat work is already almost absent, while read-only feeds reopen it for every scan. This explains why reducing feed transaction setup offers a clear read-only gain without a corresponding large mixed gain. There is no evidence here of extra synchronization causing the mixed difference. Tracing slows the load substantially and is excluded from performance claims.

SQLite's `getPageMMap` still permits mapped pages for read-only cursors inside a write transaction, but performs an additional page-cache lookup in write mode. A final experiment uses a prepared read-only `BEGIN`, wraps only the feed scans and ends the snapshot within the request, preserving the original single-post path. Its two-trial screen improves feed 12.75% and single-post 0.57%, while mixed, creation and likes change −0.79%, −1.31% and −0.50%. Those small losses do not prove a repeated regression, but they also do not establish the required no-loss result. Neither per-request snapshot is retained; the selected source and its validation identities remain unchanged.

The fdatasync screening also predates the balanced-order correction; its two-variant ordering always put fdatasync first for creation and second for mixed traffic. The apparent creation gain is therefore provisional. A separate normal synthetic-PGO build enables SQLite's Linux `HAVE_FDATASYNC=1` feature. SQLite documents `fdatasync` as adequate for data and file-size durability; this experiment keeps WAL, synchronous=NORMAL, schema and commit-before-reply behavior unchanged. The compile flag changes the SQLite archive, so these results are separate from the shared-archive transaction ablations.

Three rotated 20-second trials after ten-second warmups at 64 connections show medians of 39,710 / 39,826 feed req/s, 151,470 / 154,355 single-post req/s, 47,201 / 46,861 mixed req/s, 9,886 / 13,204 create req/s and 94,850 / 93,132 like req/s for immediate reads / immediate reads plus fdatasync. Creation improves 33.6%, but mixed falls 0.72% and likes 1.81%. The measured mixed cgroup I/O pressure deltas are approximately 13.4–14.2% of elapsed wall time for the control and 15.9–16.6% for fdatasync; CPU pressure stays below 0.3%. These include io_uring completion waits and collection-time skew, so they do not isolate storage waiting. The first fdatasync mixed trial also samples a lower mean clock, 2.57 versus 3.11 GHz, which confounds that large individual drop. These measurements do not establish either storage waiting as the main bottleneck or a mixed-throughput gain from changing the sync syscall. The fdatasync flag is not shipped.

## Measurement environment

Native MEGASERVER is an Ubuntu 24.04 / Linux 6.8.0-124 host with a Xeon E5-2690 v3, 12 physical cores / 24 threads and 64 GiB RAM. Each server container has one CPU of quota, CPU 23, 2 GiB RAM and no swap. Clients use separate physical cores and exclude both threads of the server core for capacity tests. These CPU sets constrain the benchmark; the application never sets affinity or changes kernel parameters. The organizer's separate-client DigitalOcean run remains necessary for an official score.

Measured release builds use Rust 1.94.0, SQLite 3.53.4, native instructions and the normal synthetic SQLite PGO training. The unmodified C comparator is revision `35cc55c85fa837cd703367a740e0b8eeb6b825b4`, built from source with its PGO script. Both SQLite/C builds use GCC 12.2 in the pinned Debian Bookworm builder, rather than Ubuntu's default GCC 13. The published Rust comparator is revision `398637babf1f1aa8e8d23f02eb68e1550f1fd905`. No builds run during timing. Docker seccomp is disabled equally for every variant because its default profile blocks io_uring.

Final comparisons use loopback inside a dedicated server network namespace. Clients join that namespace. No sysctl, firewall or global networking settings are changed. A preceding host-network 300,000-user probe stalled at 262,044 connected users against the host's 262,144 connection-tracking limit and is excluded as infrastructure failure. Isolating benchmark networking avoids charging those flows to the host's tracking table and protects unrelated services.

Each measured trial uses a fresh fixture copied inside the server cgroup. The destination database file cache and socket/kernel allocations therefore count against the server's memory limit. Seed-file read pages can remain charged to the host. Process RSS, anonymous memory, kernel socket memory and total cgroup memory are reported separately where available; process RSS is not total server memory.

After verifying downloads of the final measurements, raw captures and matching symbols, the owned benchmark directory, containers and images were removed from MEGASERVER. Unrelated database and Redis containers remained running. Cleanup and verification logs are retained with the evidence.

The challenge's `bench/load.js` is run unchanged with k6 2.3.0. Larger diagnostic searches use a modified, attributed derivative of C #26's load generator, with live feed IDs, ordinary think times, like/create probabilities, a 60-second total request timeout including connect, a 30-second ramp-down, ten-second grace, and connected/distinct-user accounting. Its pass decision requires aggregate errors below 1%, aggregate p95 below 500 ms, aggregate p99 below one second, and a successful feed response for every requested user. Hold-window metrics and zero-error counts are reported separately. It differs from k6 in RNG, histograms and generated post text; those runs are reported separately and cannot establish an official k6 score.
