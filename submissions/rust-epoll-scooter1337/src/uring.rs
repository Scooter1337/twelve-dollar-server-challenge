//! Completion-based Linux I/O. Fixed descriptors belong to each ring, rather than
//! the process file table. SQLite remains serialized behind one batch mutex.
use crate::{App, Parsed, error, frame};
use io_uring::{IoUring, cqueue, opcode, squeue, types};
use std::{
    alloc::{Layout, alloc, dealloc, handle_alloc_error, realloc},
    collections::VecDeque,
    io,
    net::TcpListener,
    os::fd::AsRawFd,
    ptr::{self, NonNull},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MAX_WORKERS: usize = 8;
const TARGET_SLOTS: usize = MAX_WORKERS * 65535;
const NBUFS: usize = 1024;
const BUFSIZE: usize = 2048;
const INPUT_LIMIT: usize = 1_114_112;
const OUTPUT_HIGH: usize = 65536;
const GROUP: u16 = 0;
const ACCEPT: u64 = 1;
const RECV: u64 = 2;
const SEND: u64 = 3;
const IGNORE: u64 = 4;
const LIVE: u32 = 1;
const RECEIVING: u32 = 2;
const SENDING: u32 = 4;
const CLOSING: u32 = 8;
const SHUT: u32 = 16;
const PAUSED: u32 = 32;
const DIRTY: u32 = 64;
const CONTINUED: u32 = 128;
const READY: u32 = 256;
const INDEX_SHIFT: u32 = 10;
const FLAG_MASK: u32 = (1 << INDEX_SHIFT) - 1;

#[derive(Clone, Copy)]
pub struct Tuning {
    pub batch: usize,
    pub wait_us: u32,
}
impl Default for Tuning {
    fn default() -> Self {
        Self {
            batch: 32,
            wait_us: 250,
        }
    }
}

/// A compact owning buffer. Its allocation never moves while a send is in flight.
/// Keeping lengths as u32 makes a complete connection fit one 64-byte cache line.
#[repr(C)]
#[derive(Default)]
struct Buffer {
    ptr: *mut u8,
    len: u32,
    cap: u32,
}
impl Buffer {
    fn bytes(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        // reserve initializes exactly the written prefix before increasing len.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len as usize) }
    }
    fn reserve(&mut self, needed: usize) {
        if needed <= self.cap as usize {
            return;
        }
        assert!(needed <= 4 * 1024 * 1024);
        let cap = needed.max(4096).next_power_of_two();
        let layout = Layout::from_size_align(cap, 8).unwrap();
        // Both allocation paths use the same alignment and allocator. No caller
        // may grow a Buffer after it has been transferred into Client::flight.
        let p = unsafe {
            if self.cap == 0 {
                alloc(layout)
            } else {
                realloc(
                    self.ptr,
                    Layout::from_size_align(self.cap as usize, 8).unwrap(),
                    cap,
                )
            }
        };
        if p.is_null() {
            handle_alloc_error(layout);
        }
        self.ptr = p;
        self.cap = cap as u32;
    }
    fn extend(&mut self, bytes: &[u8]) {
        let old = self.len as usize;
        self.reserve(old + bytes.len());
        if !bytes.is_empty() {
            // Source and destination are distinct allocations (or stack bytes).
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr.add(old), bytes.len());
            }
        }
        self.len = (old + bytes.len()) as u32;
    }
    fn consume(&mut self, n: usize) {
        assert!(n <= self.len as usize);
        let left = self.len as usize - n;
        if left != 0 {
            // Moving a partial/pipelined tail can overlap within the allocation.
            unsafe {
                ptr::copy(self.ptr.add(n), self.ptr, left);
            }
        }
        self.len = left as u32;
    }
}
impl Drop for Buffer {
    fn drop(&mut self) {
        if self.cap != 0 {
            unsafe {
                dealloc(
                    self.ptr,
                    Layout::from_size_align(self.cap as usize, 8).unwrap(),
                );
            }
        }
    }
}
#[repr(C)]
#[derive(Default)]
struct Client {
    input: Buffer,
    output: Buffer,
    flight: Buffer,
    sent: u32,
    generation: u32,
    touched: u32,
    state: u32,
}
const _: () = assert!(std::mem::size_of::<Client>() == 64);

struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
}
impl Mapping {
    fn new(len: usize) -> io::Result<Self> {
        let p = unsafe {
            libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            ptr: NonNull::new(p.cast()).unwrap(),
            len,
        })
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr.as_ptr().cast(), self.len);
        }
    }
}
struct Pending {
    fd: usize,
    generation: u32,
    start: u32,
    count: u32,
    interim: bool,
}
struct Worker {
    // Destroy the ring before any of the allocations referenced by its entries.
    ring: IoUring,
    recv_memory: Mapping,
    buf_ring: Option<Mapping>,
    tail: u16,
    returned: Vec<u16>,
    clients: Vec<Client>,
    slots: usize,
    reserve: usize,
    listener: i32,
    accepting: bool,
    accept_generation: u32,
    open: usize,
    ready: VecDeque<(usize, u32)>,
    completions: Vec<(u64, i32, u32)>,
    pending: Vec<Pending>,
    pool: Vec<Buffer>,
    body: Vec<u8>,
    start: Instant,
    now: u32,
    tuning: Tuning,
}
fn tag(op: u64, fd: usize, generation: u32) -> u64 {
    op | ((fd as u64) << 8) | ((generation as u64) << 32)
}
impl Worker {
    fn new(listener: i32, slots: usize, tuning: Tuning) -> io::Result<Self> {
        let ring = IoUring::builder()
            .setup_cqsize(8192)
            .setup_coop_taskrun()
            .setup_single_issuer()
            .setup_defer_taskrun()
            .build(1024)?;
        ring.submitter().register_files_sparse(slots as u32)?;
        let recv_memory = Mapping::new(NBUFS * BUFSIZE)?;
        let descriptors = Mapping::new(NBUFS * std::mem::size_of::<types::BufRingEntry>())?;
        // mmap supplies page alignment. Both mappings outlive every receive and
        // are private to this issuer; the kernel owns each published buffer until
        // its completion transfers it back to us.
        let registered = unsafe {
            ring.submitter().register_buf_ring_with_flags(
                descriptors.ptr.as_ptr() as u64,
                NBUFS as u16,
                GROUP,
                0,
            )
        }
        .is_ok();
        let mut w = Self {
            ring,
            recv_memory,
            buf_ring: registered.then_some(descriptors),
            tail: 0,
            returned: Vec::with_capacity(NBUFS),
            clients: Vec::new(),
            slots,
            reserve: 1024.min(slots / 8),
            listener,
            accepting: false,
            accept_generation: 0,
            open: 0,
            ready: VecDeque::new(),
            completions: Vec::with_capacity(8192),
            pending: Vec::with_capacity(1024),
            pool: Vec::with_capacity(256),
            body: Vec::with_capacity(16384),
            start: Instant::now(),
            now: 0,
            tuning,
        };
        if w.buf_ring.is_some() {
            for bid in 0..NBUFS {
                w.recycle(bid as u16);
            }
            w.publish_buffers();
        } else {
            w.queue(
                opcode::ProvideBuffers::new(
                    w.recv_memory.ptr.as_ptr(),
                    BUFSIZE as i32,
                    NBUFS as u16,
                    GROUP,
                    0,
                )
                .build()
                .user_data(IGNORE),
            );
            w.ring.submit_and_wait(1)?;
            let result = w
                .ring
                .completion()
                .next()
                .ok_or_else(|| io::Error::other("missing provided-buffer completion"))?
                .result();
            if result < 0 {
                return Err(io::Error::from_raw_os_error(-result));
            }
        }
        Ok(w)
    }
    fn queue(&mut self, entry: squeue::Entry) {
        loop {
            // Every pointer in an SQE refers to a mapping or a flight Buffer
            // retained until its final CQE. The SQE itself is copied into the SQ.
            if unsafe { self.ring.submission().push(&entry) }.is_ok() {
                break;
            }
            self.ring.submit().expect("submit full I/O queue");
        }
    }
    fn accept(&mut self) {
        self.accept_generation = self.accept_generation.wrapping_add(1);
        let e = opcode::AcceptMulti::new(types::Fd(self.listener))
            .allocate_file_index(true)
            .build()
            .user_data(tag(ACCEPT, 0, self.accept_generation));
        self.queue(e);
        self.accepting = true;
    }
    fn stop_accept(&mut self) {
        self.queue(
            opcode::AsyncCancel::new(tag(ACCEPT, 0, self.accept_generation))
                .build()
                .user_data(IGNORE)
                .flags(squeue::Flags::SKIP_SUCCESS),
        );
        self.accepting = false;
    }
    fn recv(&mut self, fd: usize) {
        let e = opcode::RecvMulti::new(types::Fixed(fd as u32), GROUP)
            .build()
            .user_data(tag(RECV, fd, self.clients[fd].generation));
        self.queue(e);
        self.clients[fd].state |= RECEIVING;
    }
    fn send(&mut self, fd: usize) {
        let c = &mut self.clients[fd];
        assert!(c.sent < c.flight.len);
        let e = opcode::Send::new(
            types::Fixed(fd as u32),
            unsafe { c.flight.ptr.add(c.sent as usize) },
            c.flight.len - c.sent,
        )
        .flags(libc::MSG_NOSIGNAL)
        .build()
        .user_data(tag(SEND, fd, c.generation));
        c.state |= SENDING;
        self.queue(e);
    }
    fn recycle(&mut self, bid: u16) {
        assert!((bid as usize) < NBUFS);
        if let Some(ref descriptors) = self.buf_ring {
            let index = self.tail as usize & (NBUFS - 1);
            // This descriptor slot is outside the kernel's unconsumed interval.
            // Only a completed receive's buffer is re-published into the ring.
            let entry = unsafe {
                &mut *descriptors
                    .ptr
                    .as_ptr()
                    .cast::<types::BufRingEntry>()
                    .add(index)
            };
            entry.set_addr(
                unsafe { self.recv_memory.ptr.as_ptr().add(bid as usize * BUFSIZE) } as u64,
            );
            entry.set_len(BUFSIZE as u32);
            entry.set_bid(bid);
            self.tail = self.tail.wrapping_add(1);
        } else {
            self.returned.push(bid);
        }
    }
    fn publish_buffers(&mut self) {
        if let Some(ref descriptors) = self.buf_ring {
            // Release publishes initialized descriptors after all reads of their
            // old contents have finished. The u16 tail is naturally aligned.
            let tail = unsafe { types::BufRingEntry::tail(descriptors.ptr.as_ptr().cast()) };
            unsafe {
                (&*tail.cast::<AtomicU16>()).store(self.tail, Ordering::Release);
            }
        } else {
            let mut i = 0;
            while i < self.returned.len() {
                let first = self.returned[i];
                let mut n = 1;
                while i + n < self.returned.len()
                    && self.returned[i + n] as usize == first as usize + n
                {
                    n += 1;
                }
                let p = unsafe { self.recv_memory.ptr.as_ptr().add(first as usize * BUFSIZE) };
                self.queue(
                    opcode::ProvideBuffers::new(p, BUFSIZE as i32, n as u16, GROUP, first)
                        .build()
                        .user_data(IGNORE)
                        .flags(squeue::Flags::SKIP_SUCCESS),
                );
                i += n;
            }
            self.returned.clear();
        }
    }
    fn dirty(&mut self, fd: usize) -> usize {
        let c = &mut self.clients[fd];
        if c.state & DIRTY != 0 {
            return (c.state >> INDEX_SHIFT) as usize;
        }
        let index = self.pending.len();
        assert!(index < 1 << (32 - INDEX_SHIFT));
        self.pending.push(Pending {
            fd,
            generation: c.generation,
            start: c.output.len,
            count: 0,
            interim: false,
        });
        c.state |= DIRTY | ((index as u32) << INDEX_SHIFT);
        index
    }
    fn output(&mut self, fd: usize) {
        if self.clients[fd].output.cap == 0 {
            self.clients[fd].output = self.pool.pop().unwrap_or_default();
        }
    }
    fn retire(&mut self, mut b: Buffer) {
        b.len = 0;
        if b.cap <= 16384 && self.pool.len() < 256 {
            self.pool.push(b);
        }
    }
    fn schedule(&mut self, fd: usize) {
        let c = &mut self.clients[fd];
        if c.state & READY == 0 {
            c.state |= READY;
            self.ready.push_back((fd, c.generation));
        }
    }
    fn pause(&mut self, fd: usize) {
        let c = &mut self.clients[fd];
        if c.state & PAUSED != 0 {
            return;
        }
        c.state |= PAUSED;
        if c.state & RECEIVING != 0 {
            let e = opcode::AsyncCancel::new(tag(RECV, fd, c.generation))
                .build()
                .user_data(IGNORE)
                .flags(squeue::Flags::SKIP_SUCCESS);
            self.queue(e);
        }
    }
    fn close(&mut self, fd: usize) {
        let c = &mut self.clients[fd];
        if c.state & LIVE == 0 || c.state & SENDING != 0 || c.output.len != 0 {
            return;
        }
        if c.state & RECEIVING != 0 {
            if c.state & SHUT == 0 {
                c.state |= SHUT;
                self.queue(
                    opcode::Shutdown::new(types::Fixed(fd as u32), libc::SHUT_RDWR)
                        .build()
                        .user_data(IGNORE)
                        .flags(squeue::Flags::SKIP_SUCCESS),
                );
            }
            return;
        }
        self.queue(
            opcode::Close::new(types::Fixed(fd as u32))
                .build()
                .user_data(IGNORE)
                .flags(squeue::Flags::SKIP_SUCCESS),
        );
        let c = &mut self.clients[fd];
        let generation = c.generation.wrapping_add(1);
        *c = Client {
            generation,
            ..Default::default()
        };
        self.open -= 1;
    }
    fn process(&mut self, fd: usize, bytes: &[u8], app: &mut App) {
        let mut saved = std::mem::take(&mut self.clients[fd].input);
        if self.clients[fd].state & PAUSED != 0 {
            if saved.len as usize + bytes.len() <= INPUT_LIMIT {
                saved.extend(bytes);
            } else {
                self.clients[fd].state |= CLOSING;
            }
            self.clients[fd].input = saved;
            return;
        }
        let input = if saved.len == 0 {
            bytes
        } else {
            if saved.len as usize + bytes.len() <= INPUT_LIMIT {
                saved.extend(bytes);
            } else {
                self.clients[fd].state |= CLOSING;
                self.close(fd);
                return;
            }
            saved.bytes()
        };
        let mut offset = 0;
        let mut count = 0;
        let initial = self.clients[fd].output.len;
        let mut closing = false;
        let mut deferred = false;
        while offset < input.len() {
            match frame(&input[offset..]) {
                Ok(Parsed::Complete(req)) => {
                    self.clients[fd].state &= !CONTINUED;
                    self.body.clear();
                    let status = app.serve(&req, &mut self.body);
                    let index = self.dirty(fd);
                    self.output(fd);
                    response(status, &self.body, req.close, &mut self.clients[fd].output);
                    self.pending[index].count += 1;
                    count += 1;
                    offset += req.consumed;
                    if req.close {
                        closing = true;
                        break;
                    }
                    if count >= 64 || self.clients[fd].output.len - initial >= 262144 {
                        deferred = offset < input.len();
                        break;
                    }
                }
                Ok(Parsed::Incomplete { expect_continue }) => {
                    if expect_continue && self.clients[fd].state & CONTINUED == 0 {
                        let index = self.dirty(fd);
                        self.output(fd);
                        self.clients[fd]
                            .output
                            .extend(b"HTTP/1.1 100 Continue\r\n\r\n");
                        self.pending[index].interim = true;
                        self.clients[fd].state |= CONTINUED;
                    }
                    break;
                }
                Err(()) => {
                    self.body.clear();
                    error(&mut self.body, 400, "malformed request");
                    let index = self.dirty(fd);
                    self.output(fd);
                    response(400, &self.body, true, &mut self.clients[fd].output);
                    self.pending[index].count += 1;
                    closing = true;
                    break;
                }
            }
        }
        if !closing && offset < input.len() {
            if saved.len == 0 {
                saved.extend(&bytes[offset..]);
            } else {
                saved.consume(offset);
            }
            self.clients[fd].input = saved;
        } else if saved.cap != 0 {
            self.retire(saved);
        }
        if closing {
            self.clients[fd].state |= CLOSING;
        }
        if self.clients[fd].output.len as usize >= OUTPUT_HIGH {
            self.pause(fd);
        }
        if deferred {
            self.schedule(fd);
        }
    }
    fn on_accept(&mut self, generation: u32, result: i32, flags: u32) {
        if !cqueue::more(flags) && generation == self.accept_generation {
            self.accepting = false;
        }
        if result < 0 {
            return;
        }
        let fd = result as usize;
        assert!(fd < self.slots);
        if self.clients.len() <= fd {
            self.clients.resize_with(
                (fd + 1).next_multiple_of(1024).min(self.slots),
                Client::default,
            );
        }
        let generation = self.clients[fd].generation;
        assert!(self.clients[fd].state & LIVE == 0);
        self.clients[fd] = Client {
            generation,
            touched: self.now,
            state: LIVE,
            ..Default::default()
        };
        self.open += 1;
        self.recv(fd);
        if self.accepting && self.slots - self.open < self.reserve {
            self.stop_accept();
        }
    }
    fn on_recv(&mut self, fd: usize, generation: u32, result: i32, flags: u32, app: &mut App) {
        let selected = cqueue::buffer_select(flags);
        let valid = fd < self.clients.len()
            && self.clients[fd].generation == generation
            && self.clients[fd].state & LIVE != 0;
        if valid {
            if !cqueue::more(flags) {
                self.clients[fd].state &= !RECEIVING;
            }
            if result > 0 {
                let bid = selected.expect("receive buffer id");
                assert!((bid as usize) < NBUFS && result as usize <= BUFSIZE);
                self.clients[fd].touched = self.now;
                if self.clients[fd].state & CLOSING == 0 {
                    // CQE transfers this buffer to the issuer. It is not returned
                    // to the kernel until process finishes borrowing its bytes.
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            self.recv_memory.ptr.as_ptr().add(bid as usize * BUFSIZE),
                            result as usize,
                        )
                    };
                    self.process(fd, bytes, app);
                }
            } else if (result != -libc::ENOBUFS && result != -libc::ECANCELED)
                || self.clients[fd].state & CLOSING != 0
            {
                self.clients[fd].state |= CLOSING;
                self.close(fd);
            }
            let c = &self.clients[fd];
            if c.state & (RECEIVING | CLOSING | PAUSED) == 0 && c.state & LIVE != 0 {
                self.recv(fd);
            }
        }
        if let Some(bid) = selected {
            self.recycle(bid);
        }
    }
    fn on_send(&mut self, fd: usize, generation: u32, result: i32) {
        if fd >= self.clients.len()
            || self.clients[fd].generation != generation
            || self.clients[fd].state & LIVE == 0
        {
            return;
        }
        let c = &mut self.clients[fd];
        c.state &= !SENDING;
        if result == -libc::EAGAIN || result == -libc::EINTR {
            self.send(fd);
            return;
        }
        c.touched = self.now;
        if result > 0 {
            c.sent += result as u32;
            assert!(c.sent <= c.flight.len);
        } else {
            c.state |= CLOSING;
            c.output.len = 0;
            c.sent = c.flight.len;
        }
        if c.sent < c.flight.len {
            self.send(fd);
            return;
        }
        c.sent = 0;
        let done = std::mem::take(&mut c.flight);
        self.retire(done);
        self.dirty(fd);
        if self.clients[fd].state & PAUSED != 0
            && (self.clients[fd].output.len as usize) < OUTPUT_HIGH / 2
        {
            self.clients[fd].state &= !PAUSED;
            if self.clients[fd].input.len != 0 {
                self.schedule(fd);
            }
            if self.clients[fd].state & (RECEIVING | CLOSING) == 0 {
                self.recv(fd);
            }
        }
    }
    fn flush(&mut self, committed: bool) {
        if !committed {
            self.body.clear();
            error(&mut self.body, 500, "internal server error");
            for p in &self.pending {
                let c = &mut self.clients[p.fd];
                if c.generation != p.generation || c.state & LIVE == 0 {
                    continue;
                }
                c.output.len = p.start;
                for i in 0..p.count {
                    response(
                        500,
                        &self.body,
                        c.state & CLOSING != 0 && i + 1 == p.count,
                        &mut c.output,
                    );
                }
                if p.interim {
                    c.output.extend(b"HTTP/1.1 100 Continue\r\n\r\n");
                }
            }
        }
        for i in 0..self.pending.len() {
            let fd = self.pending[i].fd;
            let c = &mut self.clients[fd];
            if c.generation != self.pending[i].generation {
                continue;
            }
            c.state &= FLAG_MASK & !DIRTY;
            if c.state & LIVE == 0 || c.state & SENDING != 0 {
                continue;
            }
            if c.output.len != 0 {
                c.flight = std::mem::take(&mut c.output);
                c.sent = 0;
                self.send(fd);
            } else if c.state & CLOSING != 0 {
                self.close(fd);
            }
        }
        self.pending.clear();
    }
    fn run(mut self, shared: Arc<Mutex<App>>) -> ! {
        let mut last_count = 0;
        let mut sweep = 0;
        loop {
            let timeout = if last_count >= 4 {
                types::Timespec::new().nsec(self.tuning.wait_us * 1000)
            } else {
                types::Timespec::new().sec(2)
            };
            let wait = if !self.ready.is_empty() {
                0
            } else if last_count >= 4 {
                self.tuning.batch.min(self.open.saturating_mul(2).max(1))
            } else {
                1
            };
            let result = self
                .ring
                .submitter()
                .submit_with_args(wait, &types::SubmitArgs::new().timespec(&timeout));
            if let Err(e) = result {
                if !matches!(
                    e.raw_os_error(),
                    Some(libc::ETIME | libc::EINTR | libc::EAGAIN | libc::EBUSY)
                ) {
                    eprintln!("io_uring: {e}");
                    std::process::exit(1);
                }
            }
            self.now = self.start.elapsed().as_secs() as u32;
            self.completions.clear();
            self.completions.extend(
                self.ring
                    .completion()
                    .map(|c| (c.user_data(), c.result(), c.flags())),
            );
            last_count = self.completions.len();
            let mut app = shared.lock().unwrap();
            let ready = self.ready.len().min(128);
            for _ in 0..ready {
                let (fd, generation) = self.ready.pop_front().unwrap();
                if self.clients[fd].generation != generation {
                    continue;
                }
                self.clients[fd].state &= !READY;
                if self.clients[fd].state & (LIVE | CLOSING | PAUSED) == LIVE {
                    self.process(fd, &[], &mut app);
                }
            }
            for i in 0..self.completions.len() {
                let (tag, result, flags) = self.completions[i];
                let fd = ((tag >> 8) & 0xffffff) as usize;
                let generation = (tag >> 32) as u32;
                match tag & 255 {
                    ACCEPT => self.on_accept(generation, result, flags),
                    RECV => self.on_recv(fd, generation, result, flags, &mut app),
                    SEND => self.on_send(fd, generation, result),
                    _ => {}
                }
            }
            self.publish_buffers();
            let committed = app.db.commit();
            self.flush(committed);
            app.db.checkpoint();
            drop(app);
            if !self.accepting && self.slots - self.open >= self.reserve * 2 {
                self.accept();
            }
            if self.now >= sweep + 10 {
                sweep = self.now;
                for fd in 0..self.clients.len() {
                    let c = &mut self.clients[fd];
                    // Idle keep-alives are retained; only stalled partial input
                    // or blocked output is closed after 120 seconds.
                    if c.state & LIVE != 0
                        && (c.input.len != 0 || c.flight.len != 0 || c.output.len != 0)
                        && self.now.saturating_sub(c.touched) >= 120
                    {
                        c.state |= CLOSING;
                        c.output.len = 0;
                        if c.state & SHUT == 0 {
                            c.state |= SHUT;
                            self.queue(
                                opcode::Shutdown::new(types::Fixed(fd as u32), libc::SHUT_RDWR)
                                    .build()
                                    .user_data(IGNORE)
                                    .flags(squeue::Flags::SKIP_SUCCESS),
                            );
                        }
                        self.close(fd);
                    }
                }
            }
        }
    }
}
fn response(status: u16, body: &[u8], close: bool, out: &mut Buffer) {
    out.extend(match status {
        200 => b"HTTP/1.1 200 OK\r\n",
        201 => b"HTTP/1.1 201 Created\r\n",
        400 => b"HTTP/1.1 400 Bad Request\r\n",
        401 => b"HTTP/1.1 401 Unauthorized\r\n",
        404 => b"HTTP/1.1 404 Not Found\r\n",
        503 => b"HTTP/1.1 503 Service Unavailable\r\n",
        _ => b"HTTP/1.1 500 Internal Server Error\r\n",
    });
    out.extend(b"Content-Type: application/json\r\nContent-Length: ");
    out.extend(itoa::Buffer::new().format(body.len()).as_bytes());
    out.extend(if close {
        b"\r\nConnection: close\r\n\r\n"
    } else {
        b"\r\n\r\n"
    });
    out.extend(body);
}
pub fn run(address: &str, app: App, tuning: Tuning) -> io::Result<()> {
    let mut limits = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limits) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let slots = limits.rlim_max.min(TARGET_SLOTS as libc::rlim_t) as usize;
    if slots < 4096 {
        return Err(io::Error::other(
            "descriptor hard limit must be at least 4096",
        ));
    }
    let workers = TARGET_SLOTS.div_ceil(slots).min(MAX_WORKERS);
    if limits.rlim_cur < slots as libc::rlim_t {
        limits.rlim_cur = slots as libc::rlim_t;
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limits) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    // Ring teardown after SIGKILL is asynchronous and may briefly retain an
    // accept's listener reference. Retry only address-in-use, well within the
    // challenge's 60-second startup allowance.
    let mut attempt = 0;
    let listener = loop {
        match TcpListener::bind(address) {
            Ok(listener) => break listener,
            Err(e) if e.raw_os_error() == Some(libc::EADDRINUSE) && attempt < 200 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    };
    let fd = listener.as_raw_fd();
    let enabled = 1i32;
    unsafe {
        if libc::listen(fd, 4096) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::setsockopt(
            fd,
            libc::IPPROTO_TCP,
            libc::TCP_NODELAY,
            (&enabled as *const i32).cast(),
            4,
        ) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    let shared = Arc::new(Mutex::new(app));
    for id in 0..workers {
        let app = Arc::clone(&shared);
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name(format!("ring-{id}"))
            .stack_size(1024 * 1024)
            .spawn(move || match Worker::new(fd, slots, tuning) {
                Ok(mut w) => {
                    w.accept();
                    match w.ring.submit() {
                        Ok(_) => {
                            tx.send(Ok(())).unwrap();
                            w.run(app);
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            })?;
        rx.recv().map_err(io::Error::other)??;
    }
    eprintln!(
        "listening {address}; io_uring, {workers} rings x {slots} direct descriptors; SQLite WAL/NORMAL"
    );
    loop {
        std::thread::park_timeout(Duration::from_secs(3600));
    }
}
