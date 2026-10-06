# All-submission benchmark results

Measured on 6 October 2026 on an Apple M1 Pro, using native Ubuntu 24.04 ARM64 in Docker Desktop. Every server stack had one CPU, 2 GiB RAM and no container swap; the load generator ran on separate CPUs. The initial table uses Rust revision `7b8c351` and the 13 submission pull requests available when the manifest was recorded (#1–14). Newer submissions #15–18 are not included. Later Rust optimizations are measured separately below. These are local throughput results, not official x86-64 DigitalOcean/k6 capacity scores.

## Throughput

Requests/second, median of three 15-second trials per workload, with two-second warmup, 64 keep-alive connections, fresh verified seed copies and varied execution order. Sorted by mixed throughput; each workload also has its own ranking.

| Implementation | Feed req/s | Post req/s | Mixed req/s |
|---|---:|---:|---:|
| Rust / synchronous epoll | 65,949 | 329,496 | 90,893 |
| C++ / epoll v2 #9 | 45,778 | 271,369 | 64,499 |
| C / fio-stl #5 | 45,261 | 195,680 | 63,138 |
| C++ / uWebSockets #6 | 42,938 | 235,782 | 62,886 |
| C++ / epoll v1 #8 | 39,320 | 196,110 | 59,369 |
| Bun / raw HTTP #3 | 34,431 | 147,616 | 56,375 |
| Bun / native router #4 | 32,188 | 108,975 | 46,367 |
| Ruby / Iodine #14 | 23,956 | 66,050 | 33,842 |
| Python / Granian #13 | 15,441 | 42,603 | 20,393 |
| Python / FastAPI ASGI #10 | 13,874 | 34,374 | 19,390 |
| Python / FastAPI #1 | 10,971 | 16,999 | 12,422 |
| Erlang / Cowboy #11 | 8,758 | 9,729 | 8,454 |
| Ruby / Rails Metal #12 + Nginx | 7,720 | 10,502 | 8,352 |
| Ruby / Rails #2 + Nginx | 2,212 | 2,984 | 2,636 |

## Leaders and differences

- Feed: Rust / synchronous epoll leads. Rust is +44.1% versus the fastest existing submission, C++ / epoll v2 #9.
- Post: Rust / synchronous epoll leads. Rust is +21.4% versus the fastest existing submission, C++ / epoll v2 #9.
- Mixed: Rust / synchronous epoll leads. Rust is +40.9% versus the fastest existing submission, C++ / epoll v2 #9.

Across these observed trials, even Rust's slowest result exceeded every baseline's fastest result on the same workload:

- Feed: +31.0% (Rust minimum 63,277; best baseline maximum 48,287 requests/s).
- Post: +19.6% (Rust minimum 325,645; best baseline maximum 272,244 requests/s).
- Mixed: +25.4% (Rust minimum 88,452; best baseline maximum 70,563 requests/s).

## Additional mixed comparison

One C++/epoll v2 mixed run in the full batch dropped to 53,059 requests/s with a 292.391 ms p99, versus 70,563 requests/s and 4.566 ms p99 in its first run. Several later feed measurements were also lower. The cause was not established. To check the mixed gain over shorter elapsed time, a separate three-trial comparison ran Rust and the three fastest existing native competitors after the full batch, with the same fresh seeds, resource limits, workload, warmup and 15-second measurement.

| Implementation | Median req/s | Min–max req/s | Median p95 / p99 ms |
|---|---:|---:|---:|
| Rust / synchronous epoll | 90,249 | 89,935–90,551 | 2.937 / 5.950 |
| C++ / epoll v2 #9 | 71,004 | 69,461–71,191 | 2.327 / 4.667 |
| C++ / uWebSockets #6 | 67,462 | 66,114–67,595 | 2.239 / 4.451 |
| C / fio-stl #5 | 62,183 | 46,968–62,463 | 2.896 / 5.594 |

Rust leads this follow-up by **27.1%** over C++ / epoll v2 #9. All 12 measured runs and warmups had zero errors; all four repeated official checks passed 42/42. Binaries were identical between the two batches. The full batch's mixed median gain is 40.9%; this shorter follow-up provides a separate estimate that avoids relying on the full batch's large baseline latency spike. Raw evidence is in [`bench/confirmatory-mixed.json.gz`](bench/confirmatory-mixed.json.gz).


## Latency and memory

p95/p99 are median run percentiles in milliseconds. Memory is the median from mixed runs. Summed process RSS includes the launcher and may double-count shared pages. Peak cgroup memory includes startup, seed copying, page cache and kernel memory.

| Implementation | Feed p95 / p99 ms | Post p95 / p99 ms | Mixed p95 / p99 ms | Mixed RSS sum MiB | Peak cgroup MiB |
|---|---:|---:|---:|---:|---:|
| Rust / synchronous epoll | 1.421 / 2.073 | 0.261 / 0.457 | 2.221 / 4.892 | 43.67 | 272.61 |
| C++ / epoll v2 #9 | 1.626 / 2.734 | 0.390 / 0.618 | 3.564 / 6.808 | 239.65 | 263.82 |
| C / fio-stl #5 | 1.658 / 2.350 | 0.394 / 0.493 | 2.100 / 4.408 | 61.77 | 291.56 |
| C++ / uWebSockets #6 | 1.696 / 3.089 | 0.452 / 0.659 | 2.566 / 5.261 | 62.32 | 291.00 |
| C++ / epoll v1 #8 | 1.878 / 3.244 | 0.554 / 0.811 | 2.948 / 5.949 | 59.11 | 287.42 |
| Bun / raw HTTP #3 | 2.179 / 3.618 | 0.548 / 0.798 | 2.633 / 5.321 | 84.20 | 320.64 |
| Bun / native router #4 | 2.338 / 3.686 | 0.735 / 1.104 | 2.658 / 5.535 | 101.69 | 307.19 |
| Ruby / Iodine #14 | 3.210 / 3.958 | 1.197 / 1.669 | 2.835 / 5.900 | 83.79 | 301.40 |
| Python / Granian #13 | 6.484 / 8.028 | 2.715 / 3.199 | 5.410 / 7.077 | 98.30 | 295.95 |
| Python / FastAPI ASGI #10 | 5.033 / 9.207 | 2.190 / 3.942 | 5.541 / 7.542 | 93.83 | 314.75 |
| Python / FastAPI #1 | 6.450 / 11.564 | 4.338 / 7.319 | 6.279 / 10.550 | 93.96 | 312.20 |
| Erlang / Cowboy #11 | 10.332 / 13.247 | 9.686 / 11.799 | 10.680 / 14.287 | 109.31 | 323.75 |
| Ruby / Rails Metal #12 + Nginx | 11.901 / 16.439 | 10.373 / 14.579 | 11.473 / 15.497 | 178.77 | 400.66 |
| Ruby / Rails #2 + Nginx | 44.546 / 97.651 | 34.812 / 53.103 | 29.158 / 41.411 | 215.95 | 428.64 |

## Trial spread

Minimum–maximum requests/s across the three trials. These ranges show observed variation, not a statistical confidence interval.

| Implementation | Feed | Post | Mixed |
|---|---:|---:|---:|
| Rust / synchronous epoll | 63,277–69,335 | 325,645–336,032 | 88,452–92,906 |
| C++ / epoll v2 #9 | 42,150–46,589 | 258,603–272,244 | 53,059–70,563 |
| C / fio-stl #5 | 43,280–48,287 | 188,228–197,147 | 54,935–63,545 |
| C++ / uWebSockets #6 | 39,482–44,532 | 227,697–240,671 | 58,846–67,971 |
| C++ / epoll v1 #8 | 37,159–40,580 | 195,620–201,072 | 50,971–62,808 |
| Bun / raw HTTP #3 | 34,087–35,954 | 145,619–149,821 | 50,365–57,348 |
| Bun / native router #4 | 29,769–33,183 | 107,237–112,111 | 41,390–46,553 |
| Ruby / Iodine #14 | 23,250–25,849 | 65,506–66,177 | 31,814–33,880 |
| Python / Granian #13 | 14,790–16,335 | 42,190–42,646 | 19,092–20,462 |
| Python / FastAPI ASGI #10 | 13,282–14,251 | 33,757–34,484 | 17,600–19,609 |
| Python / FastAPI #1 | 10,110–11,021 | 16,904–17,123 | 11,726–12,492 |
| Erlang / Cowboy #11 | 7,524–8,768 | 9,637–9,784 | 8,017–8,506 |
| Ruby / Rails Metal #12 + Nginx | 7,016–7,720 | 10,433–10,539 | 7,666–8,408 |
| Ruby / Rails #2 + Nginx | 1,972–2,542 | 2,966–3,263 | 2,399–2,658 |

## Correctness and integrity

All 14 implementations passed 42/42 official API checks before the final batch. The batch contains 126 measured runs.

Every final measured run and warmup reported zero socket errors/timeouts, zero HTTP errors and zero mixed-workload semantic errors.

The official seed row-content hash was verified. Application source and build flags are unchanged. The Bun installer was adapted only to select the checksum-pinned official ARM64 asset of the same runtime version. Rails uses its declared Nginx deployment; Nginx and Rails share one resource-limited cgroup. Pinned Ruby and Erlang runtimes were compiled once and reused.

Raw measured/warmup output, validation output, source references, binary hashes, compiler/runtime versions, CPU usage and per-process memory appear in [`bench/results.json.gz`](bench/results.json.gz). Only completed final batches are included here. Aborted setup runs contributed no rows. No builds ran during final measurements.

## Limits and reproduction

This establishes a ranking for the listed workloads at 64 connections on this ARM64 machine. It does not establish the official x86-64 score, performance with 15,000 active users, five-minute sustained capacity, or behaviour over a real network. The official load test has think time and different concurrency, so its ranking may differ. Three short trials provide limited evidence about variance and long-running write/checkpoint behaviour. Each implementation retains its own SQLite version and settings, so these results compare complete submissions rather than programming languages in isolation.

See [bench/README.md](bench/README.md) for the method and reproduction commands. [bench/manifest.json](bench/manifest.json) pins every baseline commit.

## Later allocation improvements

Revision `fbdbe2b` fixes malformed like routes and `Expect: 100-continue` handling. Revision `2f6000c` adds reusable JWT buffers, borrowed username/body parsing, prepared transaction statements, and insert-plus-timestamp lookup in place of `INSERT ... RETURNING`. It preserves full JSON/Unicode validation and commit-before-response behavior.

A thread-local Rust allocator counter around HTTP parsing, serving, commit and checkpoint work observed seven allocation/reallocation calls per ordinary create and four per like before the changes, and zero afterward. Each route was warmed twice and measured for 100 calls. Reads were already zero. SQLite's C allocator and socket I/O are outside this counter; escaped strings, chunked bodies, partial requests, backpressure and capacity growth can still allocate. The checked-in ignored allocation test asserts zero for ordinary warmed handlers.

The first 30-run comparison of buffer/transaction changes measured median gains of 7.5% for creates, 5.2% for likes, 2.6% for feeds, 0.5% for post reads and 0.8% for mixed traffic. Paired read/mixed results changed direction and their ranges overlapped, so this does not establish a read/mixed improvement. A separate four-run probe found 6.7% more creates/s after removing the temporary `RETURNING` result table, with mixed traffic unchanged.

The final three-trial confirmation compared the complete candidate directly against `fbdbe2b`, under the same one-CPU/2-GiB limits, 64 connections, fresh seeds, two-second warmups and 15-second measurements:

| Workload | Before req/s | After req/s | Median gain | Before min–max | After min–max | Before / after median p99 ms |
|---|---:|---:|---:|---:|---:|---:|
| Creates | 57,680 | 61,238 | +6.2% | 56,789–60,228 | 53,860–63,136 | 6.790 / 8.303 |
| Likes | 176,239 | 179,911 | +2.1% | 169,161–176,652 | 179,667–180,810 | 0.764 / 0.717 |
| Mixed | 90,383 | 90,361 | −0.02% | 88,270–91,476 | 89,237–92,343 | 4.618 / 4.561 |

Creates use real ASCII posts; likes target seed post 500000 with random seed users, so many likes are duplicates after warmup. Mixed uses live feed IDs. The create/mixed ranges overlap. One candidate create run had a **932.332 ms p99** spike and lower throughput; its other two p99s were 8.303 and 6.697 ms. The cause was not established. These results support modest write-throughput gains, not consistent tail-latency gains or an improved official capacity score.

All 52 measured runs and warmups across the three stages reported zero errors. Both versions passed 42/42 official checks in the final batch. The final candidate separately passed 77 additional checks, SIGKILL recovery, 15,000 health-validated connections and 66-second reuse, the allocation assertion, and an x86-64 source/C-binding cross-check. Final idle RSS was 10.06 MiB.

Raw runs, warmups, validation logs, runner source, source hashes and binary hashes are in [`bench/optimization-results.json.gz`](bench/optimization-results.json.gz). See [bench/README.md](bench/README.md) to reproduce the revision comparison. The original all-submission table remains evidence for the initial version; the final candidate was not rerun against every language submission.

## Official JavaScript workload locally

Ran the repository's **unmodified `bench/load.js`** with checksum-verified k6 2.3.0 ARM64. Each revision started from a fresh seed, passed 42/42 API checks, completed the prescribed 1,000-user/two-minute warmup, then kept the same server/database for a **2,500-user/five-minute hold**. Both runs used the script's default 60-second ramp-up and 30-second ramp-down, original think times, write probabilities and thresholds.

| Revision | Users held | p95 ms | p99 ms | Failed requests | Requests | Average req/s | Result |
|---|---:|---:|---:|---:|---:|---:|---|
| Before allocations (`fbdbe2b`) | 2,500 | 2.827 | 7.949 | 0% | 93,366 | 233.4 | Pass |
| Current (`b79f11f`) | 2,500 | 2.456 | 7.736 | 0% | 93,546 | 233.9 | Pass |

All request checks passed in both holds and both warmups. k6 summaries and threshold decisions include ramp-up/down and graceful stopping; these percentiles are the full-run summaries, not separately filtered steady-hold measurements. Requests/s is workload-paced by think time and is not maximum throughput.

Server limits: one CPU, 2 GiB without swap and 65,535 file descriptors. Client limits: two separate CPUs, 2,800 MiB without swap. HTTP used shared-host Linux loopback, with four loopback source addresses; no OS settings were changed. Both servers and clients stayed within their limits, with no OOMs. All builds finished before timing.

This establishes **both revisions pass 2,500 users locally**. There was one hold per revision; it does not establish a speedup, maximum capacity, or the official x86-64 DigitalOcean score. That revision comparison did not search for the user limit; a later search follows below. The official score still requires its separate load-generator machine and server.

Raw k6 summaries/logs, sampled server/client resource counters, source/binary identities, script checksum, preparation and runner source are in [`bench/official-k6-results.json.gz`](bench/official-k6-results.json.gz).

## Official workload capacity search

Application revision `230b2f8` was unchanged throughout the search. The unmodified official script and k6 2.3.0 retain the 60-second ramp, five-minute hold, 30-second ramp-down, think times, probabilities and thresholds. Each search kept the same server/database running after the 1,000-user/two-minute warmup. Percentiles and failure rates are the full-run k6 summaries, including ramps and graceful stopping.

The **M1/Docker ARM64 server passed 40,000 users**, with zero failures and p95/p99 **154.26/470.04 ms**. The 80,000-user probe was invalid: the Chisel transport was OOM-killed in the 4-GiB Docker VM; Rust remained running without OOM events. A separate 40,000-user repeat was also invalid because the aggregate SSH relay disconnected. These establish a local lower bound, not an M1 capacity ceiling. The transport bypassed Docker published-port forwarding and used 1-KiB buffers and eight loopback origin addresses; its exact patch and identities are included with the evidence.

To remove that transport, the same source was built as an unprivileged user and tested on **native Ubuntu 24.04 x86-64 / Xeon E5-2690 v3**, with Docker host networking. Rust 1.94.0 and GCC 12.2 came from the pinned `rust:1.94.0-bookworm` image. The server had **one CPU, 2 GiB without swap, and 65,535 descriptors**. k6 ran on separate physical cores of that same 64-GiB host, with eight loopback source addresses. This is a different server machine from the M1 comparison and uses same-host loopback, not the official separate-client DigitalOcean setup. No kernel settings or unrelated processes were changed. All builds finished before measurements, and the native server passed **42/42 official API checks** on its fresh seed before warmup.

The search started at the observed 40,000-user lower bound, doubled to a failing 80,000-user probe, then bisected to **250-user resolution** and repeated the highest pass for another five-minute hold:

| Users | Phase | p95 ms | p99 ms | Failed requests | Result |
|---:|---|---:|---:|---:|---|
| 40,000 | Search | 0.995 | 21.148 | 0.000% | Pass |
| 80,000 | Search | 2.317 | 22.250 | 4.699% | Fail |
| 60,000 | Search | 5.565 | 24.508 | 0.000% | Pass |
| 70,000 | Search | 2.324 | 21.403 | 1.421% | Fail |
| 65,000 | Search | 9.058 | 26.913 | 0.000% | Pass |
| 67,500 | Search | 2.604 | 22.239 | 0.552% | Pass |
| 68,750 | Search | 2.694 | 22.015 | 0.988% | Pass |
| 69,250 | Search | 2.761 | 22.216 | 1.165% | Fail |
| 69,000 | Search | 2.563 | 22.090 | 1.077% | Fail |
| 68,750 | Confirmation | 2.610 | 22.365 | 0.990% | Pass |

The confirmed benchmark boundary is **68,750 passing / 69,000 failing**. Both 68,750-user holds passed, at **0.988% and 0.990% failures**—very close to the permitted 1%. **65,000 users had zero failures**; that is the highest zero-failure count tested, not a separate search for an exact zero-failure maximum. This is a measured threshold boundary rather than a guarantee across machines or future runs. Requests/s is think-time paced, not maximum throughput, and no C++ capacity comparison was performed.

Above the connection limit, sampled descriptors reached **65,535** and k6 logged feed request timeouts; latency thresholds still passed. The application remained running throughout, with zero cgroup OOM events and a **340.27 MiB peak**. The host retained at least **31.71 GiB of available memory** during the generator samples. The current connection table is also bounded at 65,536 entries. These results identify the descriptor/connection cap as the constraint in this configuration; they do not establish the CPU or memory ceiling after lifting it.

Raw summaries/logs, resource samples, validation, source/binary/script/image hashes, transport patch and exact runner/preparation scripts are in [`bench/official-k6-capacity-results.json.gz`](bench/official-k6-capacity-results.json.gz). Invalid transport attempts are explicitly marked and excluded from the native bracket.
