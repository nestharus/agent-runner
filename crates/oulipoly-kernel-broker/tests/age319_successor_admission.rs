#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_kernel_broker::identity::PinnedProcess;
use oulipoly_kernel_broker::protocol::{FreshRecipientRequest, fresh_recipient_request_at};
use oulipoly_state::mailbox::{FreshRecipientIdentity, FreshV30Lane};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[test]
fn successor_peer() {
    if std::env::var_os("AGE319_SUCCESSOR_PEER").is_none() {
        return;
    }
    let socket = std::env::var("AGE319_SUCCESSOR_SOCKET").unwrap();
    let directory = std::env::var("AGE319_SUCCESSOR_DIR").unwrap();
    let allocation = std::env::var("AGE319_SUCCESSOR_ALLOCATION").unwrap();
    let source = std::env::var("AGE319_SUCCESSOR_SOURCE").unwrap();
    let seq: i64 = std::env::var("AGE319_SUCCESSOR_SEQ")
        .unwrap()
        .parse()
        .unwrap();
    let request = std::env::var("AGE319_SUCCESSOR_REQUEST").unwrap();
    let first = fresh_recipient_request_at(
        Path::new(&socket),
        &FreshRecipientRequest::OfferSuccessor {
            allocation_request_id: allocation.clone(),
            offer_request_id: request.clone(),
            seq,
            source_id: source.clone(),
        },
    )
    .unwrap();
    assert_eq!(first["kind"], "successor_offer");
    let duplicate = fresh_recipient_request_at(
        Path::new(&socket),
        &FreshRecipientRequest::OfferSuccessor {
            allocation_request_id: allocation,
            offer_request_id: request.clone(),
            seq,
            source_id: source,
        },
    )
    .unwrap();
    assert_eq!(duplicate["offer"], first["offer"]);
    let read = fresh_recipient_request_at(
        Path::new(&socket),
        &FreshRecipientRequest::ReadSuccessorOffer {
            offer_request_id: request.clone(),
        },
    )
    .unwrap();
    assert_eq!(read["offer"], first["offer"]);
    fs::write(
        Path::new(&directory).join("offer.json"),
        serde_json::to_vec(&first["offer"]).unwrap(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !Path::new(&directory).join("readback").exists() {
        assert!(
            Instant::now() < deadline,
            "successor readback signal absent"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let admitted = fresh_recipient_request_at(
        Path::new(&socket),
        &FreshRecipientRequest::ReadSuccessorAdmission {
            offer_request_id: request.clone(),
        },
    )
    .unwrap();
    assert_eq!(admitted["admission"]["offer"], first["offer"]);
    assert!(admitted["admission"]["admitted_at"].is_string());
    fs::write(
        Path::new(&directory).join("readback.json"),
        serde_json::to_vec(&admitted).unwrap(),
    )
    .unwrap();
}

#[test]
fn successor_sibling_peer() {
    if std::env::var_os("AGE319_SUCCESSOR_SIBLING").is_none() {
        return;
    }
    let socket = std::env::var("AGE319_SUCCESSOR_SOCKET").unwrap();
    let request = std::env::var("AGE319_SUCCESSOR_REQUEST").unwrap();
    let allocation = std::env::var("AGE319_SUCCESSOR_ALLOCATION").unwrap();
    let source = std::env::var("AGE319_SUCCESSOR_SOURCE").unwrap();
    let seq: i64 = std::env::var("AGE319_SUCCESSOR_SEQ")
        .unwrap()
        .parse()
        .unwrap();
    for read in [
        FreshRecipientRequest::ReadSuccessorOffer {
            offer_request_id: request.clone(),
        },
        FreshRecipientRequest::ReadSuccessorAdmission {
            offer_request_id: request.clone(),
        },
    ] {
        let answer = fresh_recipient_request_at(Path::new(&socket), &read).unwrap();
        assert!(answer["offer"].is_null() && answer["admission"].is_null());
    }
    assert!(
        fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::OfferSuccessor {
                allocation_request_id: allocation,
                offer_request_id: request,
                seq,
                source_id: source
            }
        )
        .is_err()
    );
}

#[test]
fn durable_exact_successor_offer_and_cross_store_readback() {
    if std::env::var_os("AGE319_SUCCESSOR_FIXTURE").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_exact_successor_offer_and_cross_store_readback",
                "--nocapture",
            ])
            .env("AGE319_SUCCESSOR_FIXTURE", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let broker_root = temp.path().join("broker");
    fs::create_dir(&broker_root).unwrap();
    fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
    let identity = FreshV30Lane::initialize_at(&broker_root).unwrap();
    let mut lane = FreshV30Lane::open_at(&broker_root).unwrap();
    let allocation = uuid::Uuid::new_v4().to_string();
    let session = lane.allocate_session(&allocation).unwrap();
    let state = Connection::open(broker_root.join("v30/state.db")).unwrap();
    let sidecar_path = broker_root.join("v30/sidecar/pid-identity.db");
    let side = Connection::open(&sidecar_path).unwrap();
    let original = process_identity(std::process::id() as i32);
    let root = uuid::Uuid::new_v4().to_string();
    let owner = uuid::Uuid::new_v4().to_string();
    let source = uuid::Uuid::new_v4().to_string();
    let attempt = uuid::Uuid::new_v4().to_string();
    let source_admission = uuid::Uuid::new_v4().to_string();
    let digest = "a".repeat(64);
    state
        .execute(
            "INSERT INTO fresh_lane_accepted_source VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'now')",
            params![
                source,
                attempt,
                source_admission,
                digest,
                identity.source_generation,
                identity.lane_id,
                root,
                owner
            ],
        )
        .unwrap();
    state
        .execute(
            "INSERT INTO fresh_lane_recipient_attachment VALUES(?1,?2,?3,?4,?5,?6,'now')",
            params![
                session.session_id,
                identity.lane_id,
                identity.source_generation,
                root,
                owner,
                serde_json::to_string(&original).unwrap()
            ],
        )
        .unwrap();
    side.execute(
        "INSERT INTO fresh_recipient_source VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            source,
            attempt,
            source_admission,
            digest,
            identity.source_generation,
            identity.lane_id,
            root,
            owner
        ],
    )
    .unwrap();
    side.execute(
        "INSERT INTO fresh_recipient_binding VALUES(?1,?2,?3,?4,?5)",
        params![
            session.session_id,
            serde_json::to_string(&original).unwrap(),
            root,
            owner,
            identity.source_generation
        ],
    )
    .unwrap();
    let payload = b"successor exact pending row";
    let sha = format!("{:x}", Sha256::digest(payload));
    let payload_path = sidecar_path
        .parent()
        .unwrap()
        .join("inbox-payloads/v1/sha256")
        .join(&sha[..2])
        .join(&sha);
    fs::create_dir_all(payload_path.parent().unwrap()).unwrap();
    fs::write(&payload_path, payload).unwrap();
    fs::set_permissions(&payload_path, fs::Permissions::from_mode(0o444)).unwrap();
    side.execute(
        "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
        state_dir,meta_path,log_path,rc_path,rc,payload_file_path,payload_sha256,
        payload_byte_len,payload_retention_policy)
        VALUES(?1,'fixture','successor','{}','now','/fresh','/fresh/meta','/fresh/log',
        '/fresh/rc',0,?2,?3,?4,'until_terminal_disposition')",
        params![
            session.session_id,
            payload_path.to_str().unwrap(),
            sha,
            payload.len() as i64
        ],
    )
    .unwrap();
    let seq = side.last_insert_rowid();
    side.execute(
        "INSERT INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            session.session_id,
            seq,
            source,
            attempt,
            sha,
            payload.len() as i64
        ],
    )
    .unwrap();
    side.execute(
        "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
         delivered_at,state_dir,meta_path,log_path,rc_path,rc,payload_file_path,
         payload_sha256,payload_byte_len,payload_retention_policy)
         VALUES(?1,'fixture','already-acked','{}','now','acked','/fresh','/fresh/meta',
         '/fresh/log','/fresh/rc',0,?2,?3,?4,'until_terminal_disposition')",
        params![
            session.session_id,
            payload_path.to_str().unwrap(),
            sha,
            payload.len() as i64
        ],
    )
    .unwrap();
    let acked_seq = side.last_insert_rowid();
    side.execute(
        "INSERT INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            session.session_id,
            acked_seq,
            source,
            attempt,
            sha,
            payload.len() as i64
        ],
    )
    .unwrap();
    drop(lane);
    let socket = temp.path().join("v30.sock");
    let runner = std::env::current_exe().unwrap();
    let mut broker = start_broker(&broker_root, &socket, &runner);
    let request = uuid::Uuid::new_v4().to_string();
    let mut peer = Command::new(&runner)
        .args(["--exact", "successor_peer", "--nocapture"])
        .env("AGE319_SUCCESSOR_PEER", "1")
        .env("AGE319_SUCCESSOR_SOCKET", &socket)
        .env("AGE319_SUCCESSOR_DIR", temp.path())
        .env("AGE319_SUCCESSOR_ALLOCATION", &allocation)
        .env("AGE319_SUCCESSOR_SOURCE", &source)
        .env("AGE319_SUCCESSOR_SEQ", seq.to_string())
        .env("AGE319_SUCCESSOR_REQUEST", &request)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !temp.path().join("offer.json").exists() {
        assert!(Instant::now() < deadline, "successor peer offer absent");
        std::thread::sleep(Duration::from_millis(20));
    }
    let offer: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path().join("offer.json")).unwrap()).unwrap();
    assert_ne!(
        offer["successor_identity"],
        serde_json::to_value(&original).unwrap()
    );
    assert_eq!(offer["seq"], seq);
    assert_eq!(offer["source_id"], source);
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::ReadSuccessorOffer {
                offer_request_id: request.clone(),
            }
        )
        .unwrap()["offer"]
            .is_null(),
        "wrong peer read offer"
    );
    let sibling = Command::new(&runner)
        .args(["--exact", "successor_sibling_peer", "--nocapture"])
        .env("AGE319_SUCCESSOR_SIBLING", "1")
        .env("AGE319_SUCCESSOR_SOCKET", &socket)
        .env("AGE319_SUCCESSOR_ALLOCATION", &allocation)
        .env("AGE319_SUCCESSOR_SOURCE", &source)
        .env("AGE319_SUCCESSOR_SEQ", seq.to_string())
        .env("AGE319_SUCCESSOR_REQUEST", &request)
        .status()
        .unwrap();
    assert!(sibling.success(), "sibling peer gained offer authority");
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::AdmitSuccessor {
                d_key: allocation.clone(),
                offer_request_id: request.clone(),
            }
        )
        .is_err(),
        "copied D without released root must refuse"
    );
    let mut lane = FreshV30Lane::open_at(&broker_root).unwrap();
    assert!(
        lane.offer_successor(
            &uuid::Uuid::new_v4().to_string(),
            &session,
            seq,
            &source,
            &original
        )
        .is_err()
    );
    assert!(
        lane.offer_successor(
            &uuid::Uuid::new_v4().to_string(),
            &session,
            seq,
            "wrong-source",
            &process_identity(peer.id() as i32)
        )
        .is_err()
    );
    assert!(
        lane.offer_successor(
            &uuid::Uuid::new_v4().to_string(),
            &session,
            seq + 99,
            &source,
            &process_identity(peer.id() as i32)
        )
        .is_err()
    );
    assert!(
        lane.offer_successor(
            &uuid::Uuid::new_v4().to_string(),
            &session,
            acked_seq,
            &source,
            &process_identity(peer.id() as i32)
        )
        .is_err(),
        "already ACKed sibling row must refuse successor offer"
    );
    let successor = process_identity(peer.id() as i32);
    // Force the first cross-store transition to stop after durable State.
    // The original F is fenced even before the sidecar can be repaired.
    side.execute_batch(
        "CREATE TRIGGER fixture_fail_successor_sidecar
        BEFORE INSERT ON fresh_successor_admission
        BEGIN SELECT RAISE(ABORT,'fixture lost sidecar write'); END;",
    )
    .unwrap();
    assert!(lane.admit_successor(&request, &original).is_err());
    assert!(
        lane.read_successor_admission(&request, &original).is_err(),
        "partial State admission cannot masquerade as complete readback"
    );
    assert!(
        lane.submit_recipient_delivery(&uuid::Uuid::new_v4().to_string(), &session, &original)
            .is_err(),
        "State-only admission must fence original F"
    );
    side.execute_batch("DROP TRIGGER fixture_fail_successor_sidecar")
        .unwrap();
    let admission = lane.admit_successor(&request, &original).unwrap();
    assert_eq!(admission.offer.generation, offer["generation"]);
    assert_eq!(
        lane.admit_successor(&request, &original).unwrap(),
        admission
    );
    assert!(
        lane.submit_recipient_delivery(&uuid::Uuid::new_v4().to_string(), &session, &original)
            .is_err(),
        "original F must be fenced by State admission"
    );
    assert!(
        lane.submit_recipient_delivery(&uuid::Uuid::new_v4().to_string(), &session, &successor)
            .is_err(),
        "successor F awaits explicit F/ACK join"
    );
    drop(lane);
    broker.kill().unwrap();
    broker.wait().unwrap();
    let mut broker = start_broker(&broker_root, &socket, &runner);
    fs::write(temp.path().join("readback"), b"go").unwrap();
    while !temp.path().join("readback.json").exists() {
        assert!(
            Instant::now() < deadline + Duration::from_secs(10),
            "restart readback absent"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(peer.wait().unwrap().success());
    assert_eq!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::ReadSuccessorAdmission {
                offer_request_id: request.clone(),
            }
        )
        .unwrap()["admission"]["offer"]["generation"],
        offer["generation"]
    );
    let side = Connection::open(&sidecar_path).unwrap();
    side.execute(
        "UPDATE mailbox SET delivered_at='acked' WHERE session_id=?1 AND seq=?2",
        params![session.session_id, seq],
    )
    .unwrap();
    let lane = FreshV30Lane::open_at(&broker_root).unwrap();
    assert!(
        lane.offer_successor(
            &uuid::Uuid::new_v4().to_string(),
            &session,
            seq,
            &source,
            &successor
        )
        .is_err(),
        "already ACKed row cannot accept a new offer"
    );
    broker.kill().unwrap();
    broker.wait().unwrap();
}

fn process_identity(pid: i32) -> FreshRecipientIdentity {
    let pinned = PinnedProcess::open(pid).unwrap();
    FreshRecipientIdentity {
        host_pid: pinned.host_pid,
        boot_id: pinned.boot_id,
        starttime_ticks: pinned.starttime_ticks,
        pidns_dev: pinned.pidns_dev,
        pidns_ino: pinned.pidns_ino,
    }
}

fn start_broker(root: &Path, socket: &Path, runner: &Path) -> Child {
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
            panic!("broker exited: {status}")
        }
        assert!(Instant::now() < deadline, "broker socket absent");
        std::thread::sleep(Duration::from_millis(20));
    }
}
