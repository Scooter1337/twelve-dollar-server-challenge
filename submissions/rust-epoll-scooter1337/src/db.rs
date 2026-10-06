//! SQLite owns all persisted data. This module confines the raw C pointers to one thread.
use std::{
    cell::Cell,
    ffi::{CString, c_char, c_int, c_void},
    ptr, slice,
};

enum Connection {}
enum Statement {}
unsafe extern "C" {
    fn sqlite3_open_v2(
        path: *const c_char,
        db: *mut *mut Connection,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut Connection) -> c_int;
    fn sqlite3_errmsg(db: *mut Connection) -> *const c_char;
    fn sqlite3_exec(
        db: *mut Connection,
        sql: *const c_char,
        cb: *const c_void,
        data: *mut c_void,
        err: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_prepare_v3(
        db: *mut Connection,
        sql: *const c_char,
        len: c_int,
        flags: u32,
        stmt: *mut *mut Statement,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_finalize(stmt: *mut Statement) -> c_int;
    fn sqlite3_step(stmt: *mut Statement) -> c_int;
    fn sqlite3_reset(stmt: *mut Statement) -> c_int;
    fn sqlite3_clear_bindings(stmt: *mut Statement) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut Statement, idx: c_int, val: i64) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut Statement,
        idx: c_int,
        val: *const c_char,
        len: c_int,
        destructor: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int;
    fn sqlite3_column_int64(stmt: *mut Statement, idx: c_int) -> i64;
    fn sqlite3_column_text(stmt: *mut Statement, idx: c_int) -> *const u8;
    fn sqlite3_column_bytes(stmt: *mut Statement, idx: c_int) -> c_int;
    fn sqlite3_changes(db: *mut Connection) -> c_int;
    fn sqlite3_last_insert_rowid(db: *mut Connection) -> i64;
    fn sqlite3_wal_hook(
        db: *mut Connection,
        cb: Option<
            unsafe extern "C" fn(*mut c_void, *mut Connection, *const c_char, c_int) -> c_int,
        >,
        data: *mut c_void,
    ) -> *mut c_void;
    fn sqlite3_wal_checkpoint_v2(
        db: *mut Connection,
        name: *const c_char,
        mode: c_int,
        log: *mut c_int,
        checkpointed: *mut c_int,
    ) -> c_int;
}
const ROW: i32 = 100;
const DONE: i32 = 101;
pub struct Stmt(*mut Statement);
impl Stmt {
    pub fn step(&mut self) -> i32 {
        unsafe { sqlite3_step(self.0) }
    }
    pub fn reset(&mut self) {
        unsafe {
            sqlite3_reset(self.0);
            sqlite3_clear_bindings(self.0);
        }
    }
    pub fn int(&self, col: i32) -> i64 {
        unsafe { sqlite3_column_int64(self.0, col) }
    }
    pub fn text(&self, col: i32) -> &[u8] {
        // SQLite guarantees the pointer until this statement is stepped/reset/finalized.
        // Borrowing self prevents those operations while the returned slice is live.
        unsafe {
            let p = sqlite3_column_text(self.0, col);
            let len = sqlite3_column_bytes(self.0, col);
            if p.is_null() {
                &[]
            } else {
                slice::from_raw_parts(p, len as usize)
            }
        }
    }
    pub fn bind_int(&mut self, idx: i32, value: i64) {
        unsafe {
            sqlite3_bind_int64(self.0, idx, value);
        }
    }
    // SQLITE_STATIC: caller keeps value alive through step AND reset. Kept private to
    // this module so callers cannot bind temporary strings to long-lived statements.
    unsafe fn bind_text(&mut self, idx: i32, value: &[u8]) {
        unsafe {
            sqlite3_bind_text(self.0, idx, value.as_ptr().cast(), value.len() as i32, None);
        }
    }
}
impl Drop for Stmt {
    fn drop(&mut self) {
        unsafe {
            sqlite3_finalize(self.0);
        }
    }
}
pub struct Db {
    feed: Stmt,
    post: Stmt,
    health: Stmt,
    insert: Stmt,
    created: Stmt,
    like: Stmt,
    exists: Stmt,
    begin_stmt: Stmt,
    commit_stmt: Stmt,
    rollback_stmt: Stmt,
    raw: *mut Connection,
    wal_frames: Box<Cell<i32>>,
    pub transaction: bool,
    pub group: bool,
}
unsafe extern "C" fn wal_hook(
    data: *mut c_void,
    _: *mut Connection,
    _: *const c_char,
    frames: i32,
) -> i32 {
    // Box allocation has a stable address and outlives the connection/hook.
    unsafe {
        (*data.cast::<Cell<i32>>()).set(frames);
    }
    0
}
impl Db {
    pub fn open(path: &str, group: bool) -> Self {
        let path = CString::new(path).expect("SQLITE_PATH contains NUL");
        let mut raw = ptr::null_mut();
        assert_eq!(
            unsafe { sqlite3_open_v2(path.as_ptr(), &mut raw, 2 | 0x8000, ptr::null()) },
            0,
            "open SQLite"
        );
        fn exec(raw: *mut Connection, sql: &str) {
            let sql = CString::new(sql).unwrap();
            let rc = unsafe {
                sqlite3_exec(
                    raw,
                    sql.as_ptr(),
                    ptr::null(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            assert_eq!(
                rc,
                0,
                "SQLite: {}",
                unsafe { std::ffi::CStr::from_ptr(sqlite3_errmsg(raw)) }.to_string_lossy()
            );
        }
        exec(
            raw,
            "PRAGMA locking_mode=EXCLUSIVE; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA mmap_size=1073741824; PRAGMA cache_size=500; PRAGMA temp_store=MEMORY; PRAGMA journal_size_limit=67108864; PRAGMA wal_autocheckpoint=0; SELECT count(*) FROM sqlite_schema;",
        );
        fn prepare(raw: *mut Connection, sql: &str) -> Stmt {
            let sql = CString::new(sql).unwrap();
            let mut stmt = ptr::null_mut();
            assert_eq!(
                unsafe { sqlite3_prepare_v3(raw, sql.as_ptr(), -1, 1, &mut stmt, ptr::null_mut()) },
                0,
                "prepare statement"
            );
            Stmt(stmt)
        }
        let select = "SELECT p.id,p.body,p.created_at,u.username,(SELECT count(*) FROM likes l WHERE l.post_id=p.id) FROM posts p JOIN users u ON u.id=p.user_id";
        let db = Self {
            feed: prepare(
                raw,
                &format!("{select} ORDER BY p.created_at DESC,p.id DESC LIMIT 20"),
            ),
            post: prepare(raw, &format!("{select} WHERE p.id=?1")),
            health: prepare(raw, "SELECT 1"),
            insert: prepare(raw, "INSERT INTO posts(user_id,body) VALUES(?1,?2)"),
            created: prepare(raw, "SELECT created_at FROM posts WHERE id=?1"),
            like: prepare(
                raw,
                "INSERT INTO likes(user_id,post_id) SELECT ?1,?2 WHERE EXISTS(SELECT 1 FROM posts WHERE id=?2) ON CONFLICT(user_id,post_id) DO NOTHING",
            ),
            exists: prepare(raw, "SELECT 1 FROM posts WHERE id=?1"),
            begin_stmt: prepare(raw, "BEGIN IMMEDIATE"),
            commit_stmt: prepare(raw, "COMMIT"),
            rollback_stmt: prepare(raw, "ROLLBACK"),
            raw,
            wal_frames: Box::new(Cell::new(0)),
            transaction: false,
            group,
        };
        unsafe {
            sqlite3_wal_hook(
                raw,
                Some(wal_hook),
                (&*db.wal_frames as *const Cell<i32>).cast_mut().cast(),
            );
        }
        db
    }
    fn transaction_step(stmt: &mut Stmt) -> bool {
        let ok = stmt.step() == DONE;
        stmt.reset();
        ok
    }
    fn begin(&mut self) -> bool {
        if !self.group || self.transaction {
            return true;
        }
        if !Self::transaction_step(&mut self.begin_stmt) {
            return false;
        }
        self.transaction = true;
        true
    }
    pub fn commit(&mut self) -> bool {
        if !self.transaction {
            return true;
        }
        let ok = Self::transaction_step(&mut self.commit_stmt);
        if !ok {
            Self::transaction_step(&mut self.rollback_stmt);
        }
        self.transaction = false;
        ok
    }
    pub fn checkpoint(&mut self) {
        if self.wal_frames.get() >= 1000 {
            let rc = unsafe {
                sqlite3_wal_checkpoint_v2(
                    self.raw,
                    ptr::null(),
                    2,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            };
            if rc == 0 {
                self.wal_frames.set(0);
            }
        }
    }
    pub fn read(&mut self, id: Option<i64>, out: &mut Vec<u8>) -> u16 {
        let stmt = if let Some(id) = id {
            self.post.bind_int(1, id);
            &mut self.post
        } else {
            &mut self.feed
        };
        out.extend_from_slice(if id.is_some() {
            b"{\"post\":"
        } else {
            b"{\"posts\":["
        });
        let mut count = 0;
        let mut rc = stmt.step();
        while rc == ROW {
            if count != 0 {
                out.push(b',');
            }
            append_post(stmt, out);
            count += 1;
            rc = stmt.step();
        }
        stmt.reset();
        if rc != DONE {
            return crate::error(out, 500, "internal server error");
        }
        if id.is_some() && count == 0 {
            return crate::error(out, 404, "post not found");
        }
        out.extend_from_slice(if id.is_some() { b"}" } else { b"]}" });
        200
    }
    pub fn health(&mut self, seconds: u64, out: &mut Vec<u8>) -> u16 {
        let ok = self.health.step() == ROW;
        self.health.reset();
        if !ok {
            out.extend_from_slice(b"{\"status\":\"degraded\",\"db\":\"unreachable\",\"error\":");
            crate::json_string(
                unsafe { std::ffi::CStr::from_ptr(sqlite3_errmsg(self.raw)) }.to_bytes(),
                out,
            );
            out.push(b'}');
            return 503;
        }
        out.extend_from_slice(b"{\"status\":\"ok\",\"db\":\"ok\",\"uptime_s\":");
        crate::number(seconds, out);
        out.push(b'}');
        200
    }
    pub fn create(&mut self, user: i64, username: &str, body: &str, out: &mut Vec<u8>) -> u16 {
        if !self.begin() {
            return crate::error(out, 500, "internal server error");
        }
        self.insert.bind_int(1, user);
        // body stays alive until reset below, including all error paths.
        unsafe {
            self.insert.bind_text(2, body.as_bytes());
        }
        let rc = self.insert.step();
        self.insert.reset();
        if rc != DONE {
            return crate::error(out, 500, "internal server error");
        }
        // Read this insert's generated ID and database-default timestamp while
        // the event loop still owns the connection and transaction. Avoid the
        // temporary result table that SQLite builds for INSERT ... RETURNING.
        let id = unsafe { sqlite3_last_insert_rowid(self.raw) };
        self.created.bind_int(1, id);
        let mut rc = self.created.step();
        if rc == ROW {
            out.extend_from_slice(b"{\"post\":{\"id\":");
            crate::number(id as u64, out);
            out.extend_from_slice(b",\"body\":");
            crate::json_string(body.as_bytes(), out);
            out.extend_from_slice(b",\"created_at\":");
            crate::json_string(self.created.text(0), out);
            out.extend_from_slice(b",\"author\":");
            crate::json_string(username.as_bytes(), out);
            out.extend_from_slice(b",\"like_count\":0}}");
            rc = self.created.step();
        }
        self.created.reset();
        if rc != DONE || out.is_empty() {
            crate::error(out, 500, "internal server error")
        } else {
            201
        }
    }
    pub fn like(&mut self, user: i64, post: i64, out: &mut Vec<u8>) -> u16 {
        if !self.begin() {
            return crate::error(out, 500, "internal server error");
        }
        self.like.bind_int(1, user);
        self.like.bind_int(2, post);
        let rc = self.like.step();
        self.like.reset();
        if rc != DONE {
            return crate::error(out, 500, "internal server error");
        }
        let inserted = unsafe { sqlite3_changes(self.raw) } == 1;
        if !inserted {
            self.exists.bind_int(1, post);
            let rc = self.exists.step();
            self.exists.reset();
            if rc != ROW {
                return crate::error(
                    out,
                    if rc == DONE { 404 } else { 500 },
                    if rc == DONE {
                        "post not found"
                    } else {
                        "internal server error"
                    },
                );
            }
        }
        out.extend_from_slice(if inserted {
            b"{\"liked\":true,\"already_liked\":false,\"post_id\":"
        } else {
            b"{\"liked\":true,\"already_liked\":true,\"post_id\":"
        });
        crate::number(post as u64, out);
        out.push(b'}');
        if inserted { 201 } else { 200 }
    }
}
impl Drop for Db {
    fn drop(&mut self) {
        self.commit();
        // Finalize before closing. Fields drop afterwards; sqlite3_finalize(NULL)
        // is safe, so null each pointer to prevent a second finalization.
        for s in [
            &mut self.feed,
            &mut self.post,
            &mut self.health,
            &mut self.insert,
            &mut self.created,
            &mut self.like,
            &mut self.exists,
            &mut self.begin_stmt,
            &mut self.commit_stmt,
            &mut self.rollback_stmt,
        ] {
            unsafe {
                sqlite3_finalize(s.0);
            }
            s.0 = ptr::null_mut();
        }
        unsafe {
            sqlite3_close(self.raw);
        }
    }
}
fn append_post(stmt: &Stmt, out: &mut Vec<u8>) {
    out.extend_from_slice(b"{\"id\":");
    crate::number(stmt.int(0) as u64, out);
    out.extend_from_slice(b",\"body\":");
    crate::json_string(stmt.text(1), out);
    out.extend_from_slice(b",\"created_at\":");
    crate::json_string(stmt.text(2), out);
    out.extend_from_slice(b",\"author\":");
    crate::json_string(stmt.text(3), out);
    out.extend_from_slice(b",\"like_count\":");
    crate::number(stmt.int(4) as u64, out);
    out.push(b'}');
}
