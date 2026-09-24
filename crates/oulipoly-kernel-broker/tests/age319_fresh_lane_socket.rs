#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{FreshV30Lane, FreshV30Session, MailboxDb};
use rusqlite::{Connection, params};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn private_broker_has_distinct_fresh_route_while_old_wal_writer_survives() {
    if std::env::var_os("AGE319_FRESH_SOCKET_CHILD").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("private_broker_has_distinct_fresh_route_while_old_wal_writer_survives")
            .arg("--nocapture")
            .env("AGE319_FRESH_SOCKET_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success(), "private broker fixture failed");
        return;
    }
    let private = tempfile::tempdir().unwrap();
    let old_root = private.path().join("old");
    let broker_root = private.path().join("broker");
    fs::create_dir(&old_root).unwrap();
    fs::create_dir(&broker_root).unwrap();
    fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
    let old_state = old_root.join("state.db");
    let old_mailbox = old_root.join("pid-identity.db");
    drop(StateDb::open(&old_state).unwrap());
    drop(MailboxDb::open(&old_mailbox).unwrap());
    let old = Connection::open(&old_mailbox).unwrap();
    insert_pending(&old, "old-session", "old-debt");

    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel();
    let old_state_for_writer = old_state.clone();
    let writer = std::thread::spawn(move || {
        let conn = Connection::open(old_state_for_writer).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; BEGIN IMMEDIATE;
            CREATE TABLE late_old_wal_debt(value TEXT NOT NULL);",
        )
        .unwrap();
        ready_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        conn.execute("INSERT INTO late_old_wal_debt VALUES('old-after-v30')", [])
            .unwrap();
        conn.execute_batch("COMMIT").unwrap();
    });
    ready_rx.recv().unwrap();
    let identity = FreshV30Lane::initialize_at(&broker_root).unwrap();
    let fresh_mailbox = broker_root.join("v30/sidecar/pid-identity.db");
    let fresh = Connection::open(&fresh_mailbox).unwrap();
    assert_eq!(
        fresh
            .query_row("SELECT count(*) FROM mailbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let socket = broker_root.join("fresh-v30.sock");
    let runner = std::env::current_exe().unwrap();
    let mut broker = start_broker(&broker_root, &socket, &runner);
    assert_eq!(request(&socket, b'i'), "entry-gate-v1 fresh-v30-closed\n");
    assert_eq!(
        request(&socket, b'I'),
        format!(
            "fresh-v30-route {} {} {}\n",
            identity.lane_id, identity.source_generation, identity.domain_id
        )
    );
    assert!(request(&socket, b'C').contains("fresh v30 effects closed"));
    let request_id = uuid::Uuid::new_v4();
    assert_eq!(
        request_with_id(&socket, b'd', request_id, true),
        "fresh-session absent\n"
    );
    // First reply is deliberately discarded after the broker commits.
    request_with_id(&socket, b'D', request_id, false);
    let response = request_with_id(&socket, b'd', request_id, true);
    let session: FreshV30Session =
        serde_json::from_str(response.strip_prefix("fresh-session ").unwrap().trim_end()).unwrap();
    assert_eq!(session.request_id, request_id.to_string());
    assert_eq!(request_with_id(&socket, b'D', request_id, true), response);
    assert!(request(&socket, b'D').contains("invalid challenged request"));
    assert!(
        request_with_id(&socket, b'D', uuid::Uuid::nil(), true)
            .contains("noncanonical or nil fresh request UUID")
    );
    assert_eq!(
        fresh
            .query_row("SELECT count(*) FROM fresh_lane_session", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(session.lane_id, identity.lane_id);
    let lane = FreshV30Lane::open_at(&broker_root).unwrap();
    lane.require_session(&session).unwrap();
    insert_pending(&fresh, "v30-primer", "new-primer");
    insert_pending(&fresh, &session.session_id, "same-handle");
    insert_pending(&old, &session.session_id, "same-handle");
    lane.require_mailbox_row(&session, 2).unwrap();
    let mut wrong = session.clone();
    wrong.source_generation = uuid::Uuid::new_v4().to_string();
    assert!(lane.require_mailbox_row(&wrong, 2).is_err());
    drop(lane);

    broker.kill().unwrap();
    broker.wait().unwrap();
    broker = start_broker(&broker_root, &socket, &runner);
    assert_eq!(request_with_id(&socket, b'd', request_id, true), response);
    assert_eq!(request_with_id(&socket, b'D', request_id, true), response);
    assert_eq!(
        request_with_id(&socket, b'd', uuid::Uuid::new_v4(), true),
        "fresh-session absent\n"
    );
    assert_eq!(
        fresh
            .query_row("SELECT count(*) FROM fresh_lane_session", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        request(&socket, b'I'),
        format!(
            "fresh-v30-route {} {} {}\n",
            identity.lane_id, identity.source_generation, identity.domain_id
        )
    );
    assert_eq!(FreshV30Lane::initialize_at(&broker_root).unwrap(), identity);
    go_tx.send(()).unwrap();
    writer.join().unwrap();
    assert_eq!(
        Connection::open(&old_state)
            .unwrap()
            .query_row("SELECT value FROM late_old_wal_debt", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "old-after-v30"
    );
    assert_eq!(
        old.query_row(
            "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        fresh
            .query_row(
                "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(
        fresh
            .query_row(
                "SELECT count(*) FROM mailbox WHERE handle='old-debt'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    broker.kill().unwrap();
    broker.wait().unwrap();
}

fn start_broker(
    root: &std::path::Path,
    socket: &std::path::Path,
    runner: &std::path::Path,
) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .arg("--serve-fresh-v30")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", root)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", runner)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if socket.exists() && UnixStream::connect(socket).is_ok() {
            return child;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("fresh broker exited before socket: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "fresh broker socket startup failed"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn request(socket: &std::path::Path, operation: u8) -> String {
    request_frame(socket, operation, None, true)
}

fn request_with_id(socket: &std::path::Path, operation: u8, id: uuid::Uuid, read: bool) -> String {
    request_frame(socket, operation, Some(id), read)
}

fn request_frame(
    socket: &std::path::Path,
    operation: u8,
    id: Option<uuid::Uuid>,
    read: bool,
) -> String {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).unwrap();
    let mut frame = [0u8; 33];
    frame[0] = operation;
    frame[1..17].copy_from_slice(&challenge);
    if let Some(id) = id {
        frame[17..33].copy_from_slice(id.as_bytes());
    }
    stream
        .write_all(&frame[..if id.is_some() { 33 } else { 17 }])
        .unwrap();
    if !read {
        return String::new();
    }
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn insert_pending(conn: &Connection, session: &str, handle: &str) {
    conn.execute(
        "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
        state_dir,meta_path,log_path,rc_path,rc)
        VALUES(?1,'fixture',?2,'{}','2026-09-24T00:00:00Z',
        '/fixture','/fixture/meta','/fixture/log','/fixture/rc',0)",
        params![session, handle],
    )
    .unwrap();
}
