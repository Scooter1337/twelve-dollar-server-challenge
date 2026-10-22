use super::*;

fn assert_rollback(events: &[bool], close: bool) {
    let mut output = Buffer::default();
    response(200, b"previous committed reply", false, &mut output);
    let mut expected = Buffer::default();
    expected.extend(output.bytes());
    let start = output.len;
    let count = events.iter().filter(|&&interim| !interim).count() as u32;
    let body = br#"{"error":"internal server error"}"#;
    let mut completed = 0;
    for &interim in events {
        if interim {
            output.extend(b"HTTP/1.1 100 Continue\r\n\r\n");
            expected.extend(b"HTTP/1.1 100 Continue\r\n\r\n");
        } else {
            completed += 1;
            // Response-looking bytes inside a body must not become an interim.
            response(201, b"HTTP/1.1 100 Continue\r\n\r\n", false, &mut output);
            response(500, body, close && completed == count, &mut expected);
        }
    }
    Pending {
        fd: 0,
        generation: 0,
        start,
        count,
        interim: events.contains(&true),
    }
    .rollback(&mut output, close, body);
    assert_eq!(output.bytes(), expected.bytes());
}

#[test]
fn failed_batch_preserves_interim_positions() {
    for events in [
        &[true, false][..],
        &[false, true][..],
        &[false, true, false][..],
        &[true, false, true, false, true][..],
        &[true][..],
        &[false, false][..],
        &[][..],
    ] {
        assert_rollback(events, false);
        assert_rollback(events, true);
    }
}

#[test]
fn failed_send_discards_tentative_responses() {
    for start in [0, 123] {
        let mut output = Buffer::default();
        Pending {
            fd: 0,
            generation: 0,
            start,
            count: 1,
            interim: true,
        }
        .rollback(&mut output, true, b"error");
        assert!(output.bytes().is_empty());
    }
}

#[test]
#[ignore = "requires a seeded SQLITE_PATH and Linux io_uring"]
fn failed_commit_preserves_continue_order_and_rolls_back_writes() {
    use crate::{Auth, Engine, Hmac, Mac, Sha256, URL_SAFE_NO_PAD, db};

    let _fixture = crate::SEEDED_TEST.lock().unwrap();
    let fixture = std::env::temp_dir().join(format!("uring-commit-{}.db", std::process::id()));
    std::fs::copy(std::env::var("SQLITE_PATH").unwrap(), &fixture).unwrap();
    let mut app = App {
        db: db::Db::open(fixture.to_str().unwrap(), true),
        auth: Auth::new("twelve-dollar-challenge"),
        start: Instant::now(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut worker = Worker::new(listener.as_raw_fd(), 4096, Tuning::default()).unwrap();
    // A previous send keeps this batch buffered so its encoded output can be
    // inspected without submitting socket I/O during the deterministic test.
    let mut client = Client {
        state: LIVE | SENDING,
        ..Default::default()
    };
    response(200, b"committed prefix", false, &mut client.output);
    let mut expected = Buffer::default();
    expected.extend(client.output.bytes());
    worker.clients.push(client);

    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
    let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"1","username":"review","exp":4102444800}"#);
    let signed = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(b"twelve-dollar-challenge").unwrap();
    mac.update(signed.as_bytes());
    let token = format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    let body = br#"{"body":"failed commit"}"#;
    let headers = format!(
        "POST /posts HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nExpect: 100-continue\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    for _ in 0..2 {
        worker.process(0, headers.as_bytes(), &mut app);
        worker.process(0, body, &mut app);
        expected.extend(b"HTTP/1.1 100 Continue\r\n\r\n");
        response(
            500,
            br#"{"error":"internal server error"}"#,
            false,
            &mut expected,
        );
    }
    let id = serde_json::from_slice::<serde_json::Value>(&worker.body).unwrap()["post"]["id"]
        .as_i64()
        .unwrap();
    worker.process(0, headers.as_bytes(), &mut app);
    expected.extend(b"HTTP/1.1 100 Continue\r\n\r\n");
    app.db.reject_commits();
    let committed = app.db.commit();
    assert!(!committed);
    worker.flush(committed);
    assert_eq!(worker.clients[0].output.bytes(), expected.bytes());
    drop(worker);
    drop(app);
    let mut reopened = db::Db::open(fixture.to_str().unwrap(), false);
    assert_eq!(reopened.read(Some(id), &mut Vec::new()), 404);
    drop(reopened);
    std::fs::remove_file(fixture).unwrap();
}

#[test]
#[ignore = "requires a seeded SQLITE_PATH and Linux io_uring"]
fn terminal_positive_receive_releases_closing_slot() {
    let _fixture = crate::SEEDED_TEST.lock().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut worker = Worker::new(listener.as_raw_fd(), 4096, Tuning::default()).unwrap();
    worker.clients.push(Client {
        state: LIVE | CLOSING | RECEIVING | SHUT,
        generation: 17,
        ..Default::default()
    });
    worker.open = 1;
    // Buffer 0, positive bytes, no F_MORE: the receive is now finished.
    worker.on_recv(0, 17, 8, 1, &mut test_app());
    assert_eq!(worker.open, 0);
    assert_eq!(worker.clients[0].state, 0);
    assert_eq!(worker.clients[0].generation, 18);
}

fn test_app() -> App {
    App {
        db: crate::db::Db::open(&std::env::var("SQLITE_PATH").unwrap(), true),
        auth: crate::Auth::new("twelve-dollar-challenge"),
        start: Instant::now(),
    }
}

#[test]
#[ignore = "requires a seeded SQLITE_PATH and Linux io_uring"]
fn receive_eof_preserves_checkpoint_deferred_post() {
    use crate::{Engine, Hmac, Mac, Sha256, URL_SAFE_NO_PAD};
    let _fixture = crate::SEEDED_TEST.lock().unwrap();
    let mut app = test_app();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut worker = Worker::new(listener.as_raw_fd(), 4096, Tuning::default()).unwrap();
    worker.clients.push(Client {
        // Hold an earlier response in flight. The deferred POST must survive
        // EOF both with and without this earlier output being outstanding.
        state: LIVE | RECEIVING | SENDING,
        ..Default::default()
    });
    worker.open = 1;
    worker.clients[0].flight.extend(b"previous response");
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256"}"#);
    let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"1","username":"half-close","exp":4102444800}"#);
    let signed = format!("{header}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(b"twelve-dollar-challenge").unwrap();
    mac.update(signed.as_bytes());
    let token = format!("{signed}.{}", URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()));
    let body = br#"{"body":"deferred half close"}"#;
    let request = format!("POST /posts HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(), std::str::from_utf8(body).unwrap());
    app.db.pause_writes_for_test(true);
    worker.process(0, request.as_bytes(), &mut app);
    assert_eq!(worker.clients[0].input.bytes(), request.as_bytes());
    assert!(worker.pending.is_empty());
    worker.on_recv(0, 0, 0, 0, &mut app);
    assert_ne!(worker.clients[0].state & READ_EOF, 0);
    assert_eq!(worker.clients[0].state & (CLOSING | RECEIVING | REARM), 0);
    // Retry while draining still preserves the full request.
    worker.process(0, &[], &mut app);
    assert_eq!(worker.clients[0].input.bytes(), request.as_bytes());
    app.db.pause_writes_for_test(false);
    worker.process(0, &[], &mut app);
    assert_eq!(worker.clients[0].input.len, 0);
    assert!(worker.clients[0].output.bytes().starts_with(b"HTTP/1.1 201"));
    assert_ne!(worker.clients[0].state & CLOSING, 0);
    assert!(app.db.commit());
    worker.flush(true);
    // Complete the earlier send, then flush the committed response. These
    // deterministic CQEs exercise the actual send/flush/close state machine.
    let previous = worker.clients[0].flight.len as i32;
    worker.on_send(0, 0, previous);
    worker.flush(true);
    assert!(worker.clients[0].flight.bytes().starts_with(b"HTTP/1.1 201"));
    assert_ne!(worker.clients[0].state & SENDING, 0);
    assert_eq!(worker.clients[0].state & (RECEIVING | REARM), 0);
    let response = worker.clients[0].flight.len as i32;
    worker.on_send(0, 0, response);
    worker.flush(true);
    assert_eq!(worker.open, 0);
    assert!(worker.rearm.is_empty());
}
