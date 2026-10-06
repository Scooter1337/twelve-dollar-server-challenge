mod allocation_check {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };
    struct Counter;
    thread_local! {
        static ACTIVE: Cell<bool> = const { Cell::new(false) };
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }
    fn count() {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            COUNT.with(|n| n.set(n.get() + 1));
        }
    }
    unsafe impl GlobalAlloc for Counter {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count();
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count();
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            count();
            unsafe { System.realloc(p, layout, size) }
        }
        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Counter = Counter;
    #[test]
    #[ignore = "requires a disposable seeded SQLITE_PATH and CHECK_TOKEN"]
    fn warmed_requests() {
        let mut app = App {
            db: db::Db::open(&std::env::var("SQLITE_PATH").unwrap(), true),
            auth: Auth::new("twelve-dollar-challenge"),
            start: Instant::now(),
        };
        let token = std::env::var("CHECK_TOKEN").unwrap();
        let mut out = Vec::with_capacity(65536);
        for (label, method, path, body) in [
            ("health", "GET", "/health", ""),
            ("feed", "GET", "/feed", ""),
            ("post", "GET", "/posts/500000", ""),
            (
                "create",
                "POST",
                "/posts",
                "{\"body\":\"allocation check\"}",
            ),
            ("like", "POST", "/posts/500000/like", ""),
        ] {
            let raw = format!(
                "{method} {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            for _ in 0..2 {
                let Parsed::Complete(req) = frame(raw.as_bytes()).unwrap() else {
                    panic!("incomplete")
                };
                out.clear();
                assert!(app.serve(&req, &mut out) < 300);
                assert!(app.db.commit());
            }
            COUNT.with(|n| n.set(0));
            ACTIVE.with(|n| n.set(true));
            for _ in 0..100 {
                let Parsed::Complete(req) = frame(raw.as_bytes()).unwrap() else {
                    panic!("incomplete")
                };
                out.clear();
                assert!(app.serve(&req, &mut out) < 300);
                assert!(app.db.commit());
                app.db.checkpoint();
            }
            ACTIVE.with(|n| n.set(false));
            let count = COUNT.with(Cell::get);
            eprintln!("{label}: {count} Rust allocation/reallocation calls / 100 warmed requests");
            assert_eq!(count, 0, "{label}");
        }
    }
}
