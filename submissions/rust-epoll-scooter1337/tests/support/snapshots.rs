// Both ignored tests use the same seeded database; serialize their fixture copies.
static SEEDED_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

mod snapshot_check {
    use super::*;

    #[test]
    #[ignore = "requires a seeded SQLITE_PATH"]
    fn read_batch_upgrades_and_commits() {
        let _fixture = SEEDED_TEST.lock().unwrap();
        let fixture = std::env::temp_dir().join(format!("read-snapshot-{}.db", std::process::id()));
        std::fs::copy(std::env::var("SQLITE_PATH").unwrap(), &fixture).unwrap();
        let id;
        let mut out = Vec::with_capacity(65536);
        {
            let mut db = db::Db::open(fixture.to_str().unwrap(), true);
            assert_eq!(db.read(None, &mut out), 200);
            assert!(db.transaction);
            out.clear();
            assert_eq!(
                db.create(1, "snapshot-test", "snapshot upgrade", &mut out),
                201
            );
            id = serde_json::from_slice::<serde_json::Value>(&out).unwrap()["post"]["id"]
                .as_i64()
                .unwrap();
            out.clear();
            assert_eq!(db.like(1, id, &mut out), 201);
            out.clear();
            assert_eq!(db.read(Some(id), &mut out), 200);
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&out).unwrap()["post"]["like_count"],
                1
            );
            assert!(db.commit());
            assert!(!db.transaction);
        }
        {
            let mut db = db::Db::open(fixture.to_str().unwrap(), false);
            out.clear();
            assert_eq!(db.read(Some(id), &mut out), 200);
            let post = serde_json::from_slice::<serde_json::Value>(&out).unwrap();
            assert_eq!(post["post"]["body"], "snapshot upgrade");
            assert_eq!(post["post"]["like_count"], 1);
            assert!(!db.transaction);
        }
        std::fs::remove_file(fixture).unwrap();
    }
}
