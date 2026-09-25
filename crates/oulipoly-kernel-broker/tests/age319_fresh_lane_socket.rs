#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_kernel_broker::registry::RootRegistry;
use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{FreshRecipientIdentity, FreshV30Lane, FreshV30Session, MailboxDb};
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
    if let Ok(socket) = std::env::var("AGE319_FRESH_OBSERVER_SOCKET") {
        let socket = std::path::Path::new(&socket);
        assert!(request(socket, b'I').starts_with("fresh-v30-route "));
        assert!(
            request_with_id(socket, b'D', uuid::Uuid::new_v4(), true)
                .contains("fresh lane requires installed Runner image")
        );
        return;
    }
    if let Ok(socket) = std::env::var("AGE319_FRESH_WRONG_PEER_SOCKET") {
        let request_id =
            uuid::Uuid::parse_str(&std::env::var("AGE319_FRESH_WRONG_PEER_REQUEST").unwrap())
                .unwrap();
        let response = request_with_id(std::path::Path::new(&socket), b'D', request_id, true);
        assert!(
            response.contains("belongs to another actor"),
            "sibling Runner unexpectedly spent child request: {response}"
        );
        return;
    }
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
    let runner = std::env::current_exe().unwrap();
    let old_socket = private.path().join("control.sock");
    let mut old_only = start_broker(&broker_root, &old_socket, &runner);
    assert_eq!(request(&old_socket, b'i'), "entry-gate-v1 legacy-open\n");
    assert!(!private.path().join("v30.sock").exists());
    old_only.kill().unwrap();
    old_only.wait().unwrap();
    let old_state = old_root.join("state.db");
    let old_mailbox = old_root.join("pid-identity.db");
    drop(StateDb::open(&old_state).unwrap());
    drop(MailboxDb::open(&old_mailbox).unwrap());
    let old = Connection::open(&old_mailbox).unwrap();
    old.execute_batch(
        "DROP TRIGGER completion_uncertain_input_preserve;
         DROP TABLE completion_uncertain_input;
         DROP INDEX idx_mailbox_deliverable_session_live;
         DROP INDEX idx_mailbox_deliverable_target_live;
         DROP INDEX idx_mailbox_deliverable_global;
         PRAGMA user_version=29;",
    )
    .unwrap();
    old.execute_batch(include_str!(
        "../../oulipoly-state/src/mailbox/migrations/0022_live_history_barrier.sql"
    ))
    .unwrap();
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
    let abandoned_stage = broker_root.join(format!(".v30-fresh-{}", uuid::Uuid::new_v4().simple()));
    fs::create_dir(&abandoned_stage).unwrap();
    fs::set_permissions(&abandoned_stage, fs::Permissions::from_mode(0o700)).unwrap();
    RootRegistry::open(&broker_root).unwrap();
    // Model a root published by the previous source version: all prior
    // identity/admission tables exist, while the additive child table does
    // not. Reopen applies only the embedded fresh-lane migration.
    Connection::open(broker_root.join("v30/state.db"))
        .unwrap()
        .execute_batch("DROP TABLE fresh_lane_child_request")
        .unwrap();
    assert_eq!(
        FreshV30Lane::open_at(&broker_root).unwrap().identity(),
        &identity
    );
    let fresh_mailbox = broker_root.join("v30/sidecar/pid-identity.db");
    let fresh = Connection::open(&fresh_mailbox).unwrap();
    assert_eq!(
        fresh
            .query_row("SELECT count(*) FROM mailbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let socket = private.path().join("v30.sock");
    let mut broker = start_broker(&broker_root, &socket, &runner);
    assert_eq!(socket_peer_pid(&socket), broker.id());
    assert_eq!(socket_peer_pid(&old_socket), broker.id());
    assert_eq!(request(&old_socket, b'i'), "entry-gate-v1 legacy-open\n");
    assert!(!request(&old_socket, b'I').starts_with("fresh-v30-route "));
    assert!(request(&old_socket, b'U').contains("error"));
    assert!(request_with_id(&old_socket, b'D', uuid::Uuid::new_v4(), true).contains("error"));
    assert_eq!(request(&socket, b'v'), "error invalid challenged request\n");
    assert_eq!(request(&socket, b'i'), "entry-gate-v1 fresh-v30-closed\n");
    assert_eq!(
        request(&socket, b'I'),
        format!(
            "fresh-v30-route {} {} {}\n",
            identity.lane_id, identity.source_generation, identity.domain_id
        )
    );
    // The shared front door is a different executable. A different image can
    // observe the lane but cannot reserve fresh State or obtain a work grant.
    let observer = private.path().join("route-observer");
    fs::copy(&runner, &observer).unwrap();
    assert!(
        Command::new(&observer)
            .arg("--exact")
            .arg("private_broker_has_distinct_fresh_route_while_old_wal_writer_survives")
            .arg("--nocapture")
            .env("AGE319_FRESH_OBSERVER_SOCKET", &socket)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        request_with_id(&socket, b'C', uuid::Uuid::new_v4(), true)
            .contains("installed Bash image absent")
    );
    assert!(request(&socket, b'e').contains("Runner-result/ACK lineage"));
    for operation in [b'Q', b'q', b'Z', b'z'] {
        assert!(
            request_with_id(&socket, operation, uuid::Uuid::new_v4(), true)
                .contains("fresh v30 effects closed"),
            "fresh socket accepted physical or cancellation opcode {operation}"
        );
    }
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
    assert_eq!(request(&old_socket, b'i'), "entry-gate-v1 legacy-open\n");
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
    let fresh_state = Connection::open(broker_root.join("v30/state.db")).unwrap();
    assert_eq!(
        fresh_state
            .query_row(
                "SELECT session_id FROM fresh_lane_session_admission WHERE request_id=?1",
                [request_id.to_string()],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
        session.session_id
    );
    let interrupted = uuid::Uuid::new_v4();
    let interrupted_session = format!("v30:{}:{}", identity.lane_id, uuid::Uuid::new_v4());
    fresh_state
        .execute(
            "INSERT INTO fresh_lane_session_admission
         (request_id,session_id,allocation_id,lane_id,source_generation,admitted_at)
         VALUES(?1,?2,?3,?4,?5,'2026-09-24T00:00:00Z')",
            params![
                interrupted.to_string(),
                interrupted_session,
                uuid::Uuid::new_v4().to_string(),
                identity.lane_id,
                identity.source_generation
            ],
        )
        .unwrap();
    assert!(request_with_id(&socket, b'd', interrupted, true).contains("D is incomplete"));
    let recovered = request_with_id(&socket, b'D', interrupted, true);
    let recovered_session: FreshV30Session =
        serde_json::from_str(recovered.strip_prefix("fresh-session ").unwrap().trim_end()).unwrap();
    assert_eq!(recovered_session.session_id, interrupted_session);
    assert_eq!(request_with_id(&socket, b'd', interrupted, true), recovered);
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
    let arbitrary = broker_root.join(".v30-fresh-not-a-stage");
    fs::create_dir(&arbitrary).unwrap();
    assert!(RootRegistry::open(&broker_root).is_err());
    fs::remove_dir(&arbitrary).unwrap();
    let fake_stage = broker_root.join(format!(".v30-fresh-{}", uuid::Uuid::new_v4().simple()));
    std::os::unix::fs::symlink(&abandoned_stage, &fake_stage).unwrap();
    assert!(RootRegistry::open(&broker_root).is_err());
    fs::remove_file(&fake_stage).unwrap();
    broker = start_broker(&broker_root, &socket, &runner);
    assert_eq!(socket_peer_pid(&socket), broker.id());
    assert_eq!(socket_peer_pid(&old_socket), broker.id());
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
        2
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

    // The new child pre-D reservation is tied to the challenged Runner peer,
    // is idempotent across broker restart, and refuses a sibling Runner image.
    let child_request = uuid::Uuid::new_v4();
    let child_invocation = uuid::Uuid::new_v4();
    oulipoly_kernel_broker::protocol::reserve_fresh_v30_child_request_at(
        &socket,
        &child_request.to_string(),
        &child_invocation.to_string(),
    )
    .unwrap();
    oulipoly_kernel_broker::protocol::reserve_fresh_v30_child_request_at(
        &socket,
        &child_request.to_string(),
        &child_invocation.to_string(),
    )
    .unwrap();
    assert!(
        oulipoly_kernel_broker::protocol::reserve_fresh_v30_child_request_at(
            &socket,
            &child_request.to_string(),
            &uuid::Uuid::new_v4().to_string(),
        )
        .is_err()
    );
    assert!(
        oulipoly_kernel_broker::protocol::reserve_fresh_v30_child_request_at(
            &socket,
            &uuid::Uuid::new_v4().to_string(),
            &uuid::Uuid::new_v4().to_string(),
        )
        .is_err(),
        "one pinned process reserved a second request"
    );
    let actor_json: String = fresh_state
        .query_row(
            "SELECT actor_identity FROM fresh_lane_child_request WHERE request_id=?1",
            [child_request.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    let actor: FreshRecipientIdentity = serde_json::from_str(&actor_json).unwrap();
    let lane = FreshV30Lane::open_at(&broker_root).unwrap();
    lane.require_child_actor(&child_request.to_string(), &actor, true)
        .unwrap();
    assert!(
        lane.require_child_actor(&uuid::Uuid::new_v4().to_string(), &actor, true)
            .is_err()
    );
    let mut forged = actor.clone();
    forged.starttime_ticks += 1;
    assert!(
        lane.require_child_actor(&child_request.to_string(), &forged, true)
            .is_err()
    );
    assert_eq!(
        fresh_state
            .query_row("SELECT count(*) FROM invocations", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "pre-D request must not masquerade as a real invocation"
    );
    drop(lane);
    let sibling = Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("private_broker_has_distinct_fresh_route_while_old_wal_writer_survives")
        .arg("--nocapture")
        .env("AGE319_FRESH_SOCKET_CHILD", "1")
        .env("AGE319_FRESH_WRONG_PEER_SOCKET", &socket)
        .env("AGE319_FRESH_WRONG_PEER_REQUEST", child_request.to_string())
        .status()
        .unwrap();
    assert!(sibling.success());
    request_with_id(&socket, b'D', child_request, false);
    let child_session = request_with_id(&socket, b'd', child_request, true);
    assert!(child_session.starts_with("fresh-session "));
    broker.kill().unwrap();
    broker.wait().unwrap();
    broker = start_broker(&broker_root, &socket, &runner);
    oulipoly_kernel_broker::protocol::reserve_fresh_v30_child_request_at(
        &socket,
        &child_request.to_string(),
        &child_invocation.to_string(),
    )
    .unwrap();
    assert_eq!(
        request_with_id(&socket, b'd', child_request, true),
        child_session
    );
    assert_eq!(
        request_with_id(&socket, b'D', child_request, true),
        child_session
    );
    broker.kill().unwrap();
    broker.wait().unwrap();
    // A broken fresh publication closes only its endpoint. The old registry
    // and control route remain readable after the same process restarts.
    fs::remove_file(broker_root.join("v30/sidecar/state-source.json")).unwrap();
    let mut old_after_fresh_failure = start_broker(&broker_root, &old_socket, &runner);
    assert_eq!(socket_peer_pid(&old_socket), old_after_fresh_failure.id());
    assert_eq!(request(&old_socket, b'i'), "entry-gate-v1 legacy-open\n");
    assert!(UnixStream::connect(&socket).is_err());
    old_after_fresh_failure.kill().unwrap();
    old_after_fresh_failure.wait().unwrap();
}

fn start_broker(
    root: &std::path::Path,
    socket: &std::path::Path,
    runner: &std::path::Path,
) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", root)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
            root.parent().unwrap().join("control.sock"),
        )
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

fn socket_peer_pid(socket: &std::path::Path) -> u32 {
    use std::os::fd::AsRawFd;
    let stream = UnixStream::connect(socket).unwrap();
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    assert_eq!(
        unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        },
        0
    );
    cred.pid as u32
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
