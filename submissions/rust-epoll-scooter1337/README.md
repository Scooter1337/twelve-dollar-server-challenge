# Rust / raw io_uring / SQLite

Direct HTTP/1.1 on $HOST:$PORT. No Nginx, HTTP framework, async executor or ORM. Requires Linux 6.1+ with io_uring enabled.

## Build and run

~~~bash
sudo bash install.sh
bash build.sh
SQLITE_PATH=/path/to/feed.db JWT_SECRET=twelve-dollar-challenge \
  HOST=0.0.0.0 PORT=80 bash start.sh
~~~

start.sh runs in the foreground using only the four environment variables in SPEC.md. Port 80 needs CAP_NET_BIND_SERVICE. Startup opens the existing database without scanning or copying its tables.

## Implementation

- Rust 1.94.0, edition 2024, native instructions, fat LTO and one codegen unit. Dependencies are pinned in Cargo.toml and Cargo.lock.
- SQLite 3.53.4, checksum-verified official source and raw C bindings. Prepared statements cache plans only. Request queries and commits share one NOMUTEX connection under a mutex. A second NOMUTEX connection belongs exclusively to the checkpoint thread. SQLITE_THREADSAFE=2 retains SQLite’s global and VFS synchronization.
- io_uring issuers use multishot direct accept/receive, fixed socket descriptors, provided buffers and batched completions. Tables adapt to the inherited hard limit: eight rings of 65,535 slots under the spec limit, or one ring of 524,280 slots with a one-million limit. Accepted sockets occupy those tables instead of ordinary process descriptors, lifting the previous connection ceiling under LimitNOFILE=65535. The process raises only its own soft limit within the inherited hard limit.
- Each ring has 4,096 receive buffers and submission entries, with 16,384 completion entries. Completed buffers are published before an intermediate full-queue submission. Depleted receives wait in a bounded retry queue; retries run after buffers are returned and responses are flushed, with budgets based on receive completions instead of resubmitting every ENOBUFS immediately.
- Completion waits target 32 entries with a 250 μs maximum at sustained load, or wait for one completion when idle. These are I/O batching controls: successful writes commit before their responses are submitted.
- One 64-byte connection record. Partial inputs allocate on demand. Output allocations remain fixed until the last send completion, then return to a bounded pool. Idle connections retain no response buffers. Generations reject stale completions after slot reuse; backpressure pauses reads until output drains.
- Every feed first discovers the newest 20 IDs. If their span is under 256, live range scans fetch rows and like counts, then restore timestamp/ID order. Sparse IDs use the original query. No seed-specific IDs, cached results or changed indexes.
- Live reads share a deferred SQLite transaction through each serialized completion batch. Writes upgrade it as needed, avoiding repeated pager setup while keeping every query live. Grouped responses wait for successful commit; failed batches replace tentative replies with errors.
- WAL, synchronous=NORMAL, foreign keys, stock unix-excl VFS, a 500-page SQLite cache and a 1 GiB mmap limit. Retained WAL size is limited to 16 MiB on reset; this does not cap an active WAL. Linux builds enable SQLite’s fdatasync path. A background PASSIVE checkpoint is requested every 4,096 additional WAL frames. At 65,536 frames, io_uring keeps serving reads while deferring writes for up to 500 ms to let checkpointing catch up. This is a soft drain threshold: after timeout, writes resume and the threshold advances; it is not a hard WAL size limit. Every successful write response still waits for its commit.
- SQLite PGO trains on a disposable synthetic file database at build time. It uses no challenge seed data and is not linked into the server. Disable it with bash build.sh --no-default-features.
- httparse, the LUFP JSON parser, hmac/sha2 and itoa handle HTTP parsing, full JSON validation, individual HS256 verification and integer formatting. serde_json is used only by tests. Ordinary fields borrow their input; JWT decode and response buffers are reused. Checked SSE2/NEON loads handle JSON escaping.

There is no application CPU affinity or kernel tuning. Every request reads SQLite while it is served. Idle keep-alives remain open; stalled partial requests or sends are shut down after 120 seconds.

Unsafe code covers C bindings, Linux I/O, vector loads and compact owning buffers. Received bytes are borrowed after their CQE transfers ownership and republished only after parsing finishes. Send allocations cannot grow, move or be freed while the kernel holds them. The connection table can grow because SQEs reference separate allocations, never connection-record addresses.

## Validation and measurements

The runtime passes all 42 official checks, 98 protocol/recovery checks (including 15,000 connections at hard LimitNOFILE=65535), six feed-range checks, six additional live-data checks, six ordinary Rust tests and seven seeded Rust tests. Regressions cover SQLite automatic rollback, terminal receive cleanup, deferred half-closed requests, commit-before-response ordering and checkpoint/reopen persistence. The allocation counter excludes SQLite's C allocator and socket I/O; partial inputs, escaped strings and buffer growth may allocate.

Measurements below use native x86 MEGASERVER, a shared host, one server CPU quota, 2 GiB/no swap, direct HTTP and 1,000,000 descriptors for every server. No CPU affinity or kernel tuning. The descriptor limit differs from the specification's 65,535, and the host OS has resources outside the server cgroup, so these are supplemental comparisons, not an official droplet capacity score. Zig #125 is frozen at `c02250f01d457a2698167eba35234ddef87d25b7`.

Unchanged `bench/load.js`, 1,000-user/two-minute warmup, 60-second ramp and 100,000-user/five-minute hold:

| Implementation | p95 ms | p99 ms | Failed requests |
|---|---:|---:|---:|
| Published version (`bad1638`) | 139.33 | 405.54 | 0 |
| This version | 117.11 | 357.78 | 0 |
| Zig #125 | 94.19 | 267.61 | 0 |

All three runs exit 0 and pass post-shutdown database integrity checks. Think times keep them near 9.3k req/s. The load container has 12 CPUs/32 GiB and multiple loopback source IPs. An earlier review-fix-only run failed p99 at 1.30 s despite zero failed requests; its repeat passed. The retained-WAL change is included in the current run above. Shared-host tail variability remains, and these runs do not establish either implementation's maximum.

Four alternating 60-second mixed wrk trials, Zig/Rust/Rust/Zig, use fresh databases, a two-second warmup, 64 connections and a separate 2 CPU/2 GiB load container:

| Implementation | Trial 1 req/s | Trial 2 req/s | Median req/s |
|---|---:|---:|---:|
| Zig #125 | 20,609.08 | 20,214.85 | 20,411.97 |
| This version | 27,055.30 | 25,865.00 | 26,460.15 |

Median mixed throughput is 29.6% higher. All four runs report zero HTTP/socket errors or timeouts, and committed post counts increase after shutdown. Full integrity scans are reserved for the longer validation runs below.

The C generator from [#26](https://github.com/arjaythedev/twelve-dollar-server-challenge/pull/26), with [#137](https://github.com/arjaythedev/twelve-dollar-server-challenge/pull/137)'s connection-close fixes and additional timeout/unfinished-request diagnostics, permits higher-user comparisons without k6's memory footprint. It preserves browsing probabilities and think times, but uses histogram percentiles, a fixed-date post body and no ramp-down; its results are separate from the unchanged JavaScript benchmark.

With a full two-minute/1,000-user warmup, 60-second ramp and five-minute C-generator hold at 275,000 users:

| Implementation | Hold p95 ms | Hold p99 ms | Counted failures | Outcome |
|---|---:|---:|---:|---|
| Review fixes, 256 MiB retained WAL | 565.87 | 2,104.44 | 0.086% | Fails latency |
| This version, 16 MiB retained WAL | 330.64 | 644.01 | 0.059% | Passes |
| Zig #125 | — | — | — | Server OOM |

Both Rust runs finish with all 275,000 connections open, zero request/connect timeouts and passing post-shutdown integrity checks. Their counted failures are unfinished requests at the generator's cutoff. Zig's server cgroup records one OOM kill; its generator records none, and post-shutdown integrity is unavailable. Zig also caps its fiber pool at 262,144 slots under the higher descriptor limit. A five-minute 300,000-user probe fails (hold p95 554.72 ms, p99 1,924.17 ms, 0.146% counted failures). 275,000 is the highest passing point tested, not an exact maximum.

The load container has 8 CPUs/8 GiB; this version's C generator peaks at 1.49 GiB. Server memory still reaches its 2 GiB budget; there is no total-RAM reduction claim. Four alternating mixed wrk trials comparing the 256/16 MiB settings yield medians of 26,327/26,287 req/s (−0.15%, within 0.5%). Individual runs vary substantially, so this is no claim of a precise throughput gain.

From the repository root:

```bash
bash test/run.sh submissions/rust-epoll-scooter1337
```

From this folder, run ordinary Rust tests with `cargo test --release --locked`. Seeded tests need a disposable database copy and a valid token:

```bash
allocation_fixture=$(mktemp -d)
cp /path/to/feed.db "$allocation_fixture/feed.db"
SQLITE_PATH="$allocation_fixture/feed.db" \
  CHECK_TOKEN="$(jq -r '.[0].token' /path/to/tokens.json)" \
  RUSTFLAGS="-C target-cpu=native" \
  cargo test --release --locked -- --ignored --skip parser_benchmark
rm -rf "$allocation_fixture"
```

Additional protocol/recovery and sparse-feed checks are in `tests/check.py` and `tests/feed_ranges.py`. Diagnostic CLI controls are `--uring-batch=N`, `--uring-wait-us=N`, `--no-group`, `--epoll` and `--epoll --spin-us=N`; `start.sh` uses production defaults.
