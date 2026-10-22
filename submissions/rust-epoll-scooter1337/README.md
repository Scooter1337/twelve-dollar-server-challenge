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
- WAL, synchronous=NORMAL, foreign keys, stock unix-excl VFS, a 500-page SQLite cache and a 1 GiB mmap limit. Linux builds enable SQLite’s fdatasync path. A background PASSIVE checkpoint is requested every 4,096 additional WAL frames. At 65,536 frames, io_uring keeps serving reads while deferring writes for up to 500 ms to let checkpointing catch up. This is a soft drain threshold: after timeout, writes resume and the threshold advances; it is not a hard WAL size limit. Every successful write response still waits for its commit.
- SQLite PGO trains on a disposable synthetic file database at build time. It uses no challenge seed data and is not linked into the server. Disable it with bash build.sh --no-default-features.
- httparse, the LUFP JSON parser, hmac/sha2 and itoa handle HTTP parsing, full JSON validation, individual HS256 verification and integer formatting. serde_json is used only by tests. Ordinary fields borrow their input; JWT decode and response buffers are reused. Checked SSE2/NEON loads handle JSON escaping.

There is no application CPU affinity or kernel tuning. Every request reads SQLite while it is served. Idle keep-alives remain open; stalled partial requests or sends are shut down after 120 seconds.

Unsafe code covers C bindings, Linux I/O, vector loads and compact owning buffers. Received bytes are borrowed after their CQE transfers ownership and republished only after parsing finishes. Send allocations cannot grow, move or be freed while the kernel holds them. The connection table can grow because SQEs reference separate allocations, never connection-record addresses.

## Validation and measurements

The submitted runtime/build/test files match the measured candidate. It passes all 42 official checks, six additional live-data checks, six ordinary Rust tests and four seeded tests covering warmed Rust allocations, read-to-write transaction upgrades, checkpoint/reopen persistence after 3,000 writes, and commit-failure rollback/response ordering. The allocation counter excludes SQLite's C allocator and socket I/O; partial inputs, escaped strings and buffer growth may allocate.

Native x86 MEGASERVER, shared host, one server CPU quota, 2 GiB/no swap, direct HTTP and 1,000,000 descriptors for every server. No CPU affinity or kernel tuning. The descriptor limit differs from the specification's 65,535, so these measurements are supplemental comparisons, not an official capacity score. Baselines are Rust #82 at `95aa405a0003599dbc7fb4dfa47a09e8343239df` and Zig #125 at `c02250f01d457a2698167eba35234ddef87d25b7`.

Two 60-second mixed wrk trials each, fresh database, two-second warmup, 64 connections and two client threads in a separate 2 CPU/2 GiB load container:

| Implementation | Trial 1 req/s | Trial 2 req/s | Median req/s |
|---|---:|---:|---:|
| Rust #82 | 11,961.93 | 12,058.16 | 12,010.05 |
| Zig #125 | 19,502.78 | 18,710.42 | 19,106.60 |
| This version | 23,684.21 | 24,165.55 | 23,924.88 |

Median throughput improves 25.2% over Zig and 99.2% over Rust #82. This version and Zig report zero HTTP/socket errors or timeouts; Rust #82 reports 232 and 192 wrk timeouts. These are sustained throughput tests, not maximum-user searches. The LUFP parser is already in #82 and is not credited as a new gain.

Both this version and Zig pass unchanged `bench/load.js`: 1,000 users for a two-minute warmup, a 60-second ramp to 100,000 users and a five-minute hold. Both k6 runs exit 0 with zero failed requests/checks. Rust records p95 99.39 ms and p99 329.30 ms; Zig records 134.89 ms and 348.63 ms. Think times keep both near 9.3k req/s. The separate load container uses 12 CPUs/32 GiB and multiple loopback source IPs. Both server cgroups reach the 2 GiB budget without OOM; this is not a total-memory reduction claim.

Both controllers exceed their 30-second shutdown timeout after exporting those completed benchmark results, so post-hold persistence/integrity output was not collected. Separate shutdown persistence/integrity checks and the seeded checkpoint/reopen test pass. This is a common passing point on MEGASERVER, not either server's maximum or the official droplet score.

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
