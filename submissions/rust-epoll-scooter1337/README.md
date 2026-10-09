# Rust / raw io_uring / SQLite

Direct HTTP/1.1 on $HOST:$PORT. No Nginx, HTTP framework, async executor or ORM. Requires Linux 6.1+ with io_uring enabled. The existing submission directory and executable names are retained.

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
- SQLite 3.53.4, checksum-verified official source and raw C bindings. Prepared statements cache plans only. One NOMUTEX connection is held under the same mutex for every query/commit/checkpoint batch. SQLite is compiled with SQLITE_THREADSAFE=0; workers never enter it concurrently.
- io_uring issuers use multishot direct accept/receive, fixed socket descriptors, provided buffers and batched completions. Tables adapt to the inherited hard limit: eight rings of 65,535 slots under the spec limit, or one ring of 524,280 slots with a one-million limit. Accepted sockets occupy those tables instead of ordinary process descriptors, lifting the previous connection ceiling under LimitNOFILE=65535. The process raises only its own soft limit within the inherited hard limit.
- Completion waits target 32 entries with a 250 μs maximum at sustained load, or wait for one completion when idle. These are I/O batching controls: successful writes commit before their responses are submitted.
- One 64-byte connection record. Partial inputs allocate on demand. Output allocations remain fixed until the last send completion, then return to a bounded pool. Idle connections retain no response buffers. Generations reject stale completions after slot reuse; backpressure pauses reads until output drains.
- Every feed first discovers the newest 20 IDs. If their span is under 256, live range scans fetch rows and like counts, then restore timestamp/ID order. Sparse IDs use the original query. No seed-specific IDs, cached results or changed indexes.
- WAL, synchronous=NORMAL, foreign keys, exclusive locking, a 500-page SQLite cache and a 1 GiB mmap limit. Grouped responses wait for successful commit. Failed batches replace tentative replies with errors. Checkpoints run after commit.
- SQLite PGO trains on a disposable synthetic file database at build time. It uses no challenge seed data and is not linked into the server. Disable it with bash build.sh --no-default-features.
- httparse, serde_json, hmac/sha2 and itoa handle parsing, full JSON validation, individual HS256 verification and integer formatting. Ordinary fields borrow their input; JWT decode and response buffers are reused. Checked SSE2/NEON loads handle JSON escaping.

There is no application CPU affinity or kernel tuning. Every request reads SQLite while it is served. Idle keep-alives remain open; stalled partial requests or sends are shut down after 120 seconds.

Unsafe code covers C bindings, Linux I/O, vector loads and compact owning buffers. Received bytes are borrowed after their CQE transfers ownership and republished only after parsing finishes. Send allocations cannot grow, move or be freed while the kernel holds them. The connection table can grow because SQEs reference separate allocations, never connection-record addresses.

## Validation and measurements

Current results, latency, memory, source identities and limitations are in [IO_URING.md](IO_URING.md). Earlier epoll comparisons and k6 searches are retained in [BENCHMARKS.md](BENCHMARKS.md).

From the challenge root:

~~~bash
bash test/run.sh submissions/rust-epoll-scooter1337
~~~

From this submission directory, the client can use 1M descriptors while the server is checked at the spec limit:

~~~bash
ulimit -n 1048576
python3 tests/check.py --seed /path/to/feed.db \
  --server target/release/twelve-rust-poll \
  --connections 80000 --server-nofile 65535
python3 tests/feed_ranges.py --seed /path/to/feed.db \
  --server target/release/twelve-rust-poll
~~~

The ignored allocation assertion measures 100 warmed ordinary requests per endpoint, including parsing, JWT verification, SQLite calls, commit and checkpoint. SQLite's C allocator and socket I/O are outside the counter. Escaped strings, chunked bodies, partial input, growth and cold connection setup can still allocate. Use a disposable seed copy because it writes data:

~~~bash
allocation_fixture=$(mktemp -d)
cp /path/to/feed.db "$allocation_fixture/feed.db"
SQLITE_PATH="$allocation_fixture/feed.db" \
  CHECK_TOKEN="$(jq -r '.[0].token' /path/to/tokens.json)" \
  RUSTFLAGS="-C target-cpu=native" \
  cargo test --release --locked -- --ignored --nocapture
rm -rf "$allocation_fixture"
~~~

Diagnostic controls add no environment variables; start.sh uses their defaults:

~~~bash
target/release/twelve-rust-poll --uring-batch=1
target/release/twelve-rust-poll --uring-wait-us=0
target/release/twelve-rust-poll --no-group
target/release/twelve-rust-poll --epoll
target/release/twelve-rust-poll --epoll --spin-us=50
~~~

The epoll path is retained for comparisons. Earlier busy-poll tests showed no reliable gain. Branchless and unchecked-index experiments are documented separately in IO_URING.md.
