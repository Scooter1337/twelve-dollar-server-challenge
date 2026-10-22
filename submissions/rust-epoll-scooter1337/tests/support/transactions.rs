use super::*;

#[test]
#[ignore = "requires a seeded SQLITE_PATH"]
fn automatic_rollback_cannot_escape_the_failed_batch() {
    let _fixture = crate::SEEDED_TEST.lock().unwrap();
    let fixture = std::env::temp_dir().join(format!("rollback-state-{}.db", std::process::id()));
    std::fs::copy(std::env::var("SQLITE_PATH").unwrap(), &fixture).unwrap();
    {
        let mut db = Db::open(fixture.to_str().unwrap(), true);
        let mut out = Vec::new();
        assert_eq!(db.read(Some(1), &mut out), 200);
        assert!(db.transaction);
        // OR ROLLBACK forces SQLite itself to end the transaction when the
        // seeded primary key conflicts. FULL/IOERR may produce this same state,
        // but need not roll back every transaction on every SQLite/VFS path.
        let conflict = unsafe {
            sqlite3_exec(db.raw,
                c"INSERT OR ROLLBACK INTO posts(id,user_id,body) VALUES(1,1,'force rollback')".as_ptr(),
                ptr::null(), ptr::null_mut(), ptr::null_mut())
        };
        assert_eq!(conflict, 19, "must exercise a real constraint rollback");
        assert_ne!(unsafe { sqlite3_get_autocommit(db.raw) }, 0);
        assert!(db.transaction, "retain the failed batch until commit");
        out.clear();
        assert_eq!(db.create(1, "rollback", "must not persist", &mut out), 500);
        out.clear();
        assert_eq!(db.like(1, 1, &mut out), 500);
        out.clear();
        assert_eq!(db.read(Some(1), &mut out), 500);
        assert!(!db.commit());
        assert!(!db.transaction);
        out.clear();
        assert_eq!(db.create(1, "rollback", "next batch works", &mut out), 201);
        assert!(db.commit());
    }
    let connection = CString::new(fixture.to_str().unwrap()).unwrap();
    let mut raw = ptr::null_mut();
    unsafe {
        assert_eq!(sqlite3_open_v2(connection.as_ptr(), &mut raw, 2, ptr::null()), 0);
        let mut statement = ptr::null_mut();
        assert_eq!(sqlite3_prepare_v3(raw,
            c"SELECT count(*) FROM posts WHERE body='must not persist'".as_ptr(),
            -1, 0, &mut statement, ptr::null_mut()), 0);
        assert_eq!(sqlite3_step(statement), ROW);
        assert_eq!(sqlite3_column_int64(statement, 0), 0);
        sqlite3_finalize(statement);
        assert_eq!(sqlite3_close(raw), 0);
    }
    std::fs::remove_file(fixture).unwrap();
}
