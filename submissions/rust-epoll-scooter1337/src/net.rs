//! Linux epoll, one thread, bounded buffers, and no executor/framework.
use crate::{App, Parsed, error, frame, number};
use std::{
    collections::VecDeque,
    io,
    net::{SocketAddr, TcpListener},
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

pub struct Options {
    pub group: bool,
    pub spin_us: u64,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            group: true,
            spin_us: 0,
        }
    }
}
#[derive(Default)]
struct Client {
    live: bool,
    input: Vec<u8>,
    output: Vec<u8>,
    sent: usize,
    close: bool,
    ready: bool,
    touched: u64,
    epoch: u64,
    continued: bool,
}
struct Pending {
    fd: usize,
    start: usize,
    end: usize,
    close: bool,
    count: usize,
    interim: bool,
}
const MAX_FD: usize = 65536;
const MAX_BUFFER: usize = 1_114_112;
const IN: u32 = (libc::EPOLLIN | libc::EPOLLRDHUP) as u32;
const CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";
fn ctl(ep: i32, op: i32, fd: i32, flags: u32) -> io::Result<()> {
    let mut ev = libc::epoll_event {
        events: flags,
        u64: fd as u64,
    };
    if unsafe { libc::epoll_ctl(ep, op, fd, &mut ev) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn close(ep: i32, fd: usize, clients: &mut [Client]) {
    if !clients[fd].live {
        return;
    }
    unsafe {
        libc::epoll_ctl(ep, libc::EPOLL_CTL_DEL, fd as i32, std::ptr::null_mut());
        libc::close(fd as i32);
    }
    clients[fd] = Client::default();
}
fn send(fd: usize, data: &[u8]) -> Result<usize, ()> {
    loop {
        let n = unsafe {
            libc::send(
                fd as i32,
                data.as_ptr().cast(),
                data.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n >= 0 {
            return Ok(n as usize);
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EAGAIN) => return Ok(0),
            Some(libc::EINTR) => continue,
            _ => return Err(()),
        }
    }
}
fn flush(ep: i32, fd: usize, clients: &mut [Client]) -> bool {
    let c = &mut clients[fd];
    if !c.live {
        return false;
    }
    if c.sent < c.output.len() {
        match send(fd, &c.output[c.sent..]) {
            Ok(n) => c.sent += n,
            Err(()) => {
                close(ep, fd, clients);
                return false;
            }
        }
        if c.sent < c.output.len() {
            return false;
        }
    }
    c.output.clear();
    c.sent = 0;
    if c.output.capacity() > 65536 {
        c.output = Vec::new();
    }
    if c.close {
        close(ep, fd, clients);
        return false;
    }
    if ctl(ep, libc::EPOLL_CTL_MOD, fd as i32, IN).is_err() {
        close(ep, fd, clients);
        return false;
    }
    true
}
fn response(status: u16, body: &[u8], close: bool, out: &mut Vec<u8>) {
    out.extend_from_slice(match status {
        200 => b"HTTP/1.1 200 OK\r\n",
        201 => b"HTTP/1.1 201 Created\r\n",
        400 => b"HTTP/1.1 400 Bad Request\r\n",
        401 => b"HTTP/1.1 401 Unauthorized\r\n",
        404 => b"HTTP/1.1 404 Not Found\r\n",
        503 => b"HTTP/1.1 503 Service Unavailable\r\n",
        _ => b"HTTP/1.1 500 Internal Server Error\r\n",
    });
    out.extend_from_slice(b"Content-Type: application/json\r\nContent-Length: ");
    number(body.len() as u64, out);
    out.extend_from_slice(if close {
        b"\r\nConnection: close\r\n\r\n"
    } else {
        b"\r\n\r\n"
    });
    out.extend_from_slice(body);
}
pub fn run(address: &str, mut app: App, options: Options) -> io::Result<()> {
    let address: SocketAddr = address
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let listener = TcpListener::bind(address)?;
    listener.set_nonblocking(true)?;
    let listener_fd = listener.as_raw_fd();
    // listen changes only this socket's backlog; no kernel settings are modified.
    unsafe {
        libc::listen(listener_fd, 4096);
    }
    let ep = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if ep < 0 {
        return Err(io::Error::last_os_error());
    }
    ctl(ep, libc::EPOLL_CTL_ADD, listener_fd, libc::EPOLLIN as u32)?;
    let mut clients: Vec<Client> = (0..MAX_FD).map(|_| Client::default()).collect();
    let mut events = vec![libc::epoll_event { events: 0, u64: 0 }; 256];
    let mut scratch = vec![0u8; 65536];
    let mut body = Vec::with_capacity(16384);
    let mut arena = Vec::with_capacity(2 * 1024 * 1024);
    let mut pending = Vec::<Pending>::with_capacity(256);
    let mut ready = VecDeque::<usize>::new();
    let mut work = Vec::with_capacity(512);
    let mut epoch = 0u64;
    let mut hot_until = Instant::now();
    let start = Instant::now();
    let mut last_sweep = 0;
    eprintln!(
        "listening {address}; group={}, spin={}us; SQLite WAL/NORMAL",
        options.group, options.spin_us
    );
    loop {
        let timeout = if !ready.is_empty() || Instant::now() < hot_until {
            0
        } else {
            1000
        };
        let n = unsafe { libc::epoll_wait(ep, events.as_mut_ptr(), events.len() as i32, timeout) };
        if n < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if n > 0 {
            hot_until = Instant::now() + Duration::from_micros(options.spin_us);
        }
        let now = start.elapsed().as_secs();
        epoch = epoch.wrapping_add(1);
        arena.clear();
        pending.clear();
        // Buffered pipeline tails also need scheduling, even with an empty kernel
        // receive queue. Take only the current queue generation for fair batches.
        let queued = ready.len().min(256);
        work.clear();
        for _ in 0..queued {
            if let Some(fd) = ready.pop_front() {
                work.push((fd, 0u32, true));
            }
        }
        work.extend(
            events
                .iter()
                .take(n as usize)
                .map(|ev| (ev.u64 as usize, ev.events, false)),
        );
        for &(fd, flags, buffered) in &work {
            if arena.len() >= 4 * 1024 * 1024 {
                if buffered {
                    ready.push_back(fd);
                }
                continue;
            }
            if fd == listener_fd as usize {
                for _ in 0..64 {
                    let fd = unsafe {
                        libc::accept4(
                            listener_fd,
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                        )
                    };
                    if fd < 0 {
                        break;
                    }
                    if fd as usize >= MAX_FD {
                        unsafe {
                            libc::close(fd);
                        }
                        continue;
                    }
                    let enabled = 1i32;
                    unsafe {
                        libc::setsockopt(
                            fd,
                            libc::IPPROTO_TCP,
                            libc::TCP_NODELAY,
                            (&enabled as *const i32).cast(),
                            4,
                        );
                    }
                    clients[fd as usize] = Client {
                        live: true,
                        touched: now,
                        ..Default::default()
                    };
                    if ctl(ep, libc::EPOLL_CTL_ADD, fd, IN).is_err() {
                        close(ep, fd as usize, &mut clients);
                    }
                }
                continue;
            }
            if fd >= MAX_FD || !clients[fd].live {
                continue;
            }
            if clients[fd].epoch == epoch || (buffered && !clients[fd].ready) {
                continue;
            }
            clients[fd].epoch = epoch;
            if flags & libc::EPOLLERR as u32 != 0 {
                close(ep, fd, &mut clients);
                continue;
            }
            if flags & libc::EPOLLOUT as u32 != 0 {
                clients[fd].touched = now;
            }
            if flags & libc::EPOLLOUT as u32 != 0 && !flush(ep, fd, &mut clients) {
                continue;
            }
            if !clients[fd].output.is_empty() {
                continue;
            }
            if !buffered && clients[fd].ready {
                // A blocked pipeline became writable. Its buffered bytes must be
                // parsed before accepting more bytes from that same connection.
                ready.push_back(fd);
                continue;
            }
            if !buffered && flags & (libc::EPOLLIN | libc::EPOLLRDHUP | libc::EPOLLHUP) as u32 == 0
            {
                continue;
            }
            let size = if buffered {
                0
            } else {
                unsafe { libc::recv(fd as i32, scratch.as_mut_ptr().cast(), scratch.len(), 0) }
            };
            if size < 0 {
                if matches!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EAGAIN) | Some(libc::EINTR)
                ) {
                    continue;
                }
                close(ep, fd, &mut clients);
                continue;
            }
            if !buffered && size == 0 {
                close(ep, fd, &mut clients);
                continue;
            }
            clients[fd].touched = now;
            clients[fd].ready = false;
            let mut accumulated = std::mem::take(&mut clients[fd].input);
            let input = if accumulated.is_empty() {
                &scratch[..size as usize]
            } else {
                accumulated.extend_from_slice(&scratch[..size as usize]);
                &accumulated[..]
            };
            let mut offset = 0;
            let mut closing = false;
            let begin = arena.len();
            let mut count = 0;
            let mut interim = false;
            let mut deferred = false;
            if input.len() > MAX_BUFFER {
                body.clear();
                error(&mut body, 400, "malformed request");
                response(400, &body, true, &mut arena);
                closing = true;
                count = 1;
            } else {
                while offset < input.len() {
                    match frame(&input[offset..]) {
                        Ok(Parsed::Complete(req)) => {
                            clients[fd].continued = false;
                            body.clear();
                            let status = app.serve(&req, &mut body);
                            response(status, &body, req.close, &mut arena);
                            count += 1;
                            offset += req.consumed;
                            if req.close {
                                closing = true;
                                break;
                            }
                            if count >= 64 || arena.len() - begin >= 262144 {
                                deferred = offset < input.len();
                                break;
                            }
                        }
                        Ok(Parsed::Incomplete { expect_continue }) => {
                            if expect_continue && !clients[fd].continued {
                                arena.extend_from_slice(CONTINUE);
                                clients[fd].continued = true;
                                interim = true;
                            }
                            break;
                        }
                        Err(()) => {
                            body.clear();
                            error(&mut body, 400, "malformed request");
                            response(400, &body, true, &mut arena);
                            closing = true;
                            count += 1;
                            break;
                        }
                    }
                }
            }
            if !closing && offset < input.len() {
                if accumulated.is_empty() {
                    accumulated.extend_from_slice(&scratch[offset..size as usize]);
                } else {
                    accumulated.drain(..offset);
                }
                clients[fd].input = accumulated;
                clients[fd].ready = deferred;
            }
            if arena.len() != begin {
                pending.push(Pending {
                    fd,
                    start: begin,
                    end: arena.len(),
                    close: closing,
                    count,
                    interim,
                });
            }
        }
        // No byte reflecting a write (or a subsequent read of that write) leaves
        // the process until the entire batch commits. A failed COMMIT discards all
        // the batch's tentative responses while preserving HTTP pipeline framing.
        if !app.db.commit() {
            arena.clear();
            body.clear();
            error(&mut body, 500, "internal server error");
            for p in &mut pending {
                p.start = arena.len();
                for i in 0..p.count {
                    response(500, &body, p.close && i + 1 == p.count, &mut arena);
                }
                if p.interim {
                    arena.extend_from_slice(CONTINUE);
                }
                p.end = arena.len();
            }
        }
        for p in &pending {
            if !clients[p.fd].live {
                continue;
            }
            let c = &mut clients[p.fd];
            let bytes = &arena[p.start..p.end];
            match send(p.fd, bytes) {
                Ok(n) if n == bytes.len() => {
                    if p.close {
                        close(ep, p.fd, &mut clients);
                    } else if c.ready {
                        ready.push_back(p.fd);
                    }
                }
                Ok(n) => {
                    c.output.extend_from_slice(&bytes[n..]);
                    c.sent = 0;
                    c.close = p.close;
                    // Stop reading a slow client until its current response drains.
                    if ctl(ep, libc::EPOLL_CTL_MOD, p.fd as i32, libc::EPOLLOUT as u32).is_err() {
                        close(ep, p.fd, &mut clients);
                    }
                }
                Err(()) => close(ep, p.fd, &mut clients),
            }
        }
        app.db.checkpoint();
        if now >= last_sweep + 10 {
            for fd in 0..clients.len() {
                if clients[fd].live && now.saturating_sub(clients[fd].touched) >= 120 {
                    close(ep, fd, &mut clients);
                }
            }
            last_sweep = now;
        }
        if arena.capacity() > 8 * 1024 * 1024 {
            arena = Vec::with_capacity(2 * 1024 * 1024);
        }
    }
}
