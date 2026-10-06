# Rust / raw epoll / SQLite

Direct HTTP on `$HOST:$PORT`, with one synchronous event loop owning the SQLite connection. No Nginx, HTTP framework, async executor or ORM. Linux x86-64 and ARM64.

## Build and run

```bash
sudo bash install.sh
bash build.sh
SQLITE_PATH=/path/to/feed.db JWT_SECRET=twelve-dollar-challenge \
  HOST=0.0.0.0 PORT=80 bash start.sh
```

The server runs in the foreground and uses only the four environment variables from the spec. Port 80 requires `CAP_NET_BIND_SERVICE`; use port 3000 for local tests. Startup opens the existing database without scanning or copying its tables. The keep-alive timeout is 120 seconds.

From the challenge root:

```bash
bash seed/make-seed.sh
bash test/run.sh submissions/rust-epoll-scooter1337
```

## Stack and optimizations

- **Rust 1.94.0**, edition 2024, release optimization, native CPU instructions, fat LTO and one codegen unit. Runtime/build dependencies are pinned in `Cargo.toml` and `Cargo.lock`.
- **SQLite 3.53.4**, SHA256-verified official amalgamation, statically compiled with GCC at `-O3`. Raw C bindings, one `NOMUTEX` connection and ten prepared statements. Transaction statements are prepared once. Creates use an insert and a live timestamp lookup to avoid SQLite's temporary `RETURNING` result table. No Rust database wrapper.
- **Direct Linux epoll and sockets** through `libc`. `httparse` parses HTTP, `serde_json` validates request JSON, `hmac`/`sha2` verify HS256 signatures, and `itoa` serializes integers. SIMD JSON escaping uses SSE2 on x86-64 and NEON on ARM64. Explicit bounds protect vector loads.
- **One thread** avoids queues, executor dispatch and synchronization between HTTP and SQLite. Only the server's own thread is pinned to a permitted CPU. No kernel settings or other processes are changed.
- **Group commit** collects writes within an event-loop batch. Every generated response waits for a successful commit; failures roll back and replace the tentative responses with errors. There is no time-based batching delay.
- **WAL with `synchronous=NORMAL`**, foreign keys, exclusive locking, a 500-page cache and a 1 GiB mmap limit. A WAL hook schedules restart checkpoints at 1,000 frames after responses. The schema and indexes are unchanged.
- **SQLite PGO** instruments the C object, trains it against a disposable file database, then recompiles it using the profile. The trainer is not linked into the server and uses no challenge seed data. Clang and cross-compilation fall back to ordinary optimization. Disable profiling with `bash build.sh --no-default-features`.
- **Reusable buffers and bounded work** limit allocations and prevent one connection from monopolizing the loop. Only partial requests and blocked writes acquire per-connection buffers. Buffered pipeline tails are scheduled explicitly; slow readers pause input until their output drains.
- **Borrowed request data** uses reusable JWT decoding buffers, a stack signature buffer and borrowed usernames. A JSON visitor retains only the post body while validating the whole document. Warmed ordinary ASCII request handlers allocate no Rust heap memory; SQLite internals, escaped strings, chunked bodies and buffer growth can still allocate.

Every request reads live SQLite data. Prepared statements cache plans only; SQLite's page cache and mmap are the only stored-data caches. JWT signatures are verified individually. Successful writes reach `SQLITE_DONE` and wait for commit before their responses are sent. Unsafe code is limited to SQLite bindings, Linux syscalls and checked SIMD loads.

Busy polling is optional. Local tests found no meaningful throughput benefit, so the default is blocking epoll. Comparison controls do not add environment variables:

```bash
target/release/twelve-rust-poll --spin-us=50
target/release/twelve-rust-poll --spin-us=200
target/release/twelve-rust-poll --no-group
target/release/twelve-rust-poll --no-pin
```

## Validation and results

Passed all 42 official checks and 77 additional checks covering auth, Unicode, borrowed JSON/JWT parsing, fragmented/chunked HTTP, `100 Continue`, malformed like routes/framing, duplicate concurrent likes, 1,800 pipelined responses with backpressure, and reads following writes. Acknowledged writes survived `SIGKILL` and restart. Builds run as an unprivileged user; the initial installation was also tested on fresh Ubuntu.

Validated 15,000 simultaneous idle connections and reused 200 original sockets after 66 seconds. Process RSS was 10.06 MiB at that idle connection count. This is separate from throughput testing and excludes kernel socket memory. The source and SQLite bindings also cross-check for x86-64; x86-64 execution has not been measured.

The initial version (`7b8c351`) had the highest feed, post and mixed medians in the local comparison of 13 pinned submissions (#1–14). Newer submissions #15–18 are not included. Against C++/epoll v2 #9:

| Requests/s | C++ #9 | Rust | Gain |
|---|---:|---:|---:|
| Feed | 45,778 | 65,949 | 44.1% |
| Single post | 271,369 | 329,496 | 21.4% |
| Mixed confirmation | 71,004 | 90,249 | 27.1% |

Ubuntu 24.04 ARM64 / Docker Desktop / Apple M1 Pro; one CPU and 2 GiB without swap per stack, load generation on separate CPUs. Three 15-second trials, fresh seeds, two-second warmup and 64 keep-alive connections. Mixed uses a separate confirmation batch because the larger batch had more variation. Mixed p99 was 5.950 ms versus C++'s 4.667 ms. All 138 measured runs and warmups reported zero errors.

These are local throughput comparisons, not the official x86-64 DigitalOcean/k6 capacity score. Full rankings, memory, latency, trial ranges, pinned references and reproduction instructions are in [BENCHMARKS.md](BENCHMARKS.md).

The later allocation changes increased create throughput from 57,680 to 61,238 requests/s (+6.2%) and likes from 176,239 to 179,911 (+2.1%) in a separate three-trial confirmation against the corrected pre-optimization version. Mixed throughput was unchanged. Create/mixed ranges overlap, and one create run had a 932 ms p99 spike; these are throughput estimates, not a claim of consistent latency improvement.

```bash
ulimit -n 65535
python3 tests/check.py --seed /path/to/fresh-seed.db \
  --server target/release/twelve-rust-poll
```

The optional allocation assertion parses and serves 100 warmed requests per endpoint, including commit/checkpoint work. It counts Rust allocator calls on the calling thread, excluding SQLite's C allocator and socket I/O. Use a disposable seed copy because it writes posts and likes:

```bash
allocation_fixture=$(mktemp -d)
cp /path/to/fresh-seed.db "$allocation_fixture/feed.db"
SQLITE_PATH="$allocation_fixture/feed.db" \
  CHECK_TOKEN="$(jq -r '.[0].token' /path/to/tokens.json)" \
  RUSTFLAGS="-C target-cpu=native" \
  cargo test --release --locked -- --ignored --nocapture
rm -rf "$allocation_fixture"
```
