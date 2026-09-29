#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use oulipoly_kernel_broker::identity::PinnedProcess;
use oulipoly_kernel_broker::protocol::{FreshRecipientRequest, fresh_recipient_request_at};
use oulipoly_state::mailbox::{FreshRecipientIdentity, FreshV30Lane};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
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
    if std::env::var_os("AGE319_SUCCESSOR_FACK").is_some() {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !Path::new(&directory).join("f-go").exists() {
            assert!(Instant::now() < deadline, "successor F signal absent");
            std::thread::sleep(Duration::from_millis(20));
        }
        let delivery_request_id = uuid::Uuid::new_v4().to_string();
        let submit = FreshRecipientRequest::SubmitSuccessor {
            offer_request_id: request.clone(),
            delivery_request_id: delivery_request_id.clone(),
        };
        // A full request with a deliberately lost reply cannot create a second grant.
        let mut stream = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        stream.read_exact(&mut challenge).unwrap();
        let mut frame = vec![b'F'];
        frame.extend_from_slice(&challenge);
        frame.extend_from_slice(&serde_json::to_vec(&submit).unwrap());
        stream.write_all(&frame).unwrap();
        drop(stream);
        let read = loop {
            let read = fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::ReadSuccessorDelivery {
                    delivery_request_id: delivery_request_id.clone(),
                },
            );
            match read {
                Ok(value) if !value["delivery"].is_null() => break value,
                _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                other => panic!("lost successor F readback absent: {other:?}"),
            }
        };
        assert!(fresh_recipient_request_at(Path::new(&socket), &submit).is_err());
        let recovered = fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::RecoverSuccessorDelivery {
                delivery_request_id: delivery_request_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(recovered["delivery"], read["delivery"]);
        assert_eq!(
            recovered["delivery"]["generation"],
            first["offer"]["generation"]
        );
        let token = recovered["delivery_token"].as_str().unwrap().to_owned();
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::AcknowledgeSuccessor {
                    delivery_request_id: delivery_request_id.clone(),
                    delivery_token: token.clone()
                }
            )
            .is_err(),
            "ACK without receiver receipt"
        );
        fs::write(
            Path::new(&directory).join("f.json"),
            serde_json::to_vec(&recovered).unwrap(),
        )
        .unwrap();
        while !Path::new(&directory).join("f-check").exists() {
            assert!(Instant::now() < deadline + Duration::from_secs(15));
            std::thread::sleep(Duration::from_millis(20));
        }
        let actor: FreshRecipientIdentity =
            serde_json::from_value(first["offer"]["successor_identity"].clone()).unwrap();
        let payload = oulipoly_kernel_broker::protocol::persist_successor_receiver_receipt(
            &recovered, &actor,
        )
        .unwrap();
        assert_eq!(
            oulipoly_kernel_broker::protocol::persist_successor_receiver_receipt(
                &recovered, &actor
            )
            .unwrap(),
            payload
        );
        fs::write(Path::new(&directory).join("received.bin"), &payload).unwrap();
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::AcknowledgeSuccessor {
                    delivery_request_id: delivery_request_id.clone(),
                    delivery_token: token.clone()
                }
            )
            .is_err(),
            "ACK without certified receipt"
        );
        let certified = fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::CertifySuccessorReceipt {
                delivery_request_id: delivery_request_id.clone(),
            },
        )
        .unwrap();
        let receipt_read = fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::ReadSuccessorReceipt {
                delivery_request_id: delivery_request_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(certified["receipt_sha256"], receipt_read["receipt_sha256"]);
        let receipt_path = std::path::PathBuf::from(recovered["receipt_path"].as_str().unwrap());
        let held = receipt_path.with_extension("held");
        fs::rename(&receipt_path, &held).unwrap();
        let mut wrong: serde_json::Value =
            serde_json::from_slice(&fs::read(&held).unwrap()).unwrap();
        wrong["source_id"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
        fs::write(&receipt_path, serde_json::to_vec(&wrong).unwrap()).unwrap();
        fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::AcknowledgeSuccessor {
                    delivery_request_id: delivery_request_id.clone(),
                    delivery_token: token.clone()
                }
            )
            .is_err(),
            "changed receiver receipt authorized ACK"
        );
        fs::remove_file(&receipt_path).unwrap();
        fs::rename(&held, &receipt_path).unwrap();
        fs::File::open(receipt_path.parent().unwrap())
            .unwrap()
            .sync_all()
            .unwrap();
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::AcknowledgeSuccessor {
                    delivery_request_id: delivery_request_id.clone(),
                    delivery_token: uuid::Uuid::new_v4().to_string()
                }
            )
            .is_err(),
            "wrong token ACK"
        );
        let mut stream = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        stream.read_exact(&mut challenge).unwrap();
        let mut frame = vec![b'F'];
        frame.extend_from_slice(&challenge);
        frame.extend_from_slice(
            &serde_json::to_vec(&FreshRecipientRequest::AcknowledgeSuccessor {
                delivery_request_id: delivery_request_id.clone(),
                delivery_token: token.clone(),
            })
            .unwrap(),
        );
        stream.write_all(&frame).unwrap();
        drop(stream);
        let ack = loop {
            let answer = fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::ReadSuccessorAck {
                    delivery_request_id: delivery_request_id.clone(),
                },
            );
            match answer {
                Ok(value) if !value["ack"].is_null() => break value,
                _ if Instant::now() < deadline + Duration::from_secs(15) => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                other => panic!("lost successor ACK readback absent: {other:?}"),
            }
        };
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::AcknowledgeSuccessor {
                    delivery_request_id: delivery_request_id.clone(),
                    delivery_token: token
                }
            )
            .is_err(),
            "duplicate token ACK"
        );
        let ack_read = fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::ReadSuccessorAck {
                delivery_request_id: delivery_request_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(ack["ack"], ack_read["ack"]);
        fs::write(
            Path::new(&directory).join("fack.json"),
            serde_json::to_vec(&serde_json::json!({
                "delivery_request_id":delivery_request_id,"ack":ack["ack"],
                "receipt_sha256":certified["receipt_sha256"]
            }))
            .unwrap(),
        )
        .unwrap();
        while !Path::new(&directory).join("restart-ack").exists() {
            assert!(Instant::now() < deadline + Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(20));
        }
        let after = fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::ReadSuccessorAck {
                delivery_request_id: delivery_request_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(after["ack"], ack["ack"]);
        fs::write(
            Path::new(&directory).join("restart-ack.json"),
            serde_json::to_vec(&after).unwrap(),
        )
        .unwrap();
    }
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
                offer_request_id: request.clone(),
                seq,
                source_id: source
            }
        )
        .is_err()
    );
    if std::env::var_os("AGE319_SUCCESSOR_FACK").is_some() {
        let delivery_request_id = std::env::var("AGE319_SUCCESSOR_DELIVERY_REQUEST").unwrap();
        let token = std::env::var("AGE319_SUCCESSOR_TOKEN").unwrap();
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::SubmitSuccessor {
                    offer_request_id: request.clone(),
                    delivery_request_id: uuid::Uuid::new_v4().to_string()
                }
            )
            .is_err()
        );
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::ReadSuccessorDelivery {
                    delivery_request_id: delivery_request_id.clone()
                }
            )
            .unwrap()["delivery"]
                .is_null()
        );
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::RecoverSuccessorDelivery {
                    delivery_request_id: delivery_request_id.clone()
                }
            )
            .is_err()
        );
        assert!(
            fresh_recipient_request_at(
                Path::new(&socket),
                &FreshRecipientRequest::AcknowledgeSuccessor {
                    delivery_request_id,
                    delivery_token: token
                }
            )
            .is_err()
        );
    }
}

#[test]
fn durable_exact_successor_offer_and_cross_store_readback() {
    run_successor_fixture(false);
}

#[test]
fn durable_exact_successor_fack_and_cross_store_readback() {
    run_successor_fixture(true);
}

fn run_successor_fixture(fack: bool) {
    if std::env::var_os("AGE319_SUCCESSOR_FIXTURE").is_none() {
        let name = if fack {
            "durable_exact_successor_fack_and_cross_store_readback"
        } else {
            "durable_exact_successor_offer_and_cross_store_readback"
        };
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
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
    // Reopen a published v30 admission lane that predates the delivery slice.
    drop(lane);
    side.execute_batch(
        "DROP TABLE fresh_successor_ack_evidence;
         DROP TABLE fresh_successor_receipt;
         DROP TABLE fresh_successor_grant;",
    )
    .unwrap();
    let lane = FreshV30Lane::open_at(&broker_root).unwrap();
    assert_eq!(
        lane.read_session(&allocation).unwrap(),
        Some(session.clone())
    );
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
        .envs(fack.then_some(("AGE319_SUCCESSOR_FACK", "1")))
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
    if fack {
        assert!(
            fresh_recipient_request_at(
                &socket,
                &FreshRecipientRequest::SubmitSuccessor {
                    offer_request_id: request.clone(),
                    delivery_request_id: uuid::Uuid::new_v4().to_string()
                }
            )
            .is_err(),
            "copied offer ID authorized original/wrong peer"
        );
        fs::write(temp.path().join("f-go"), b"go").unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !temp.path().join("f.json").exists() {
            assert!(Instant::now() < deadline, "successor F recovery absent");
            std::thread::sleep(Duration::from_millis(20));
        }
        let f: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("f.json")).unwrap()).unwrap();
        let delivery_request_id = f["delivery_request_id"].as_str().unwrap();
        let token = f["delivery_token"].as_str().unwrap();
        assert!(
            fresh_recipient_request_at(
                &socket,
                &FreshRecipientRequest::AcknowledgeSuccessor {
                    delivery_request_id: delivery_request_id.into(),
                    delivery_token: token.into()
                }
            )
            .is_err(),
            "original/wrong peer ACKed with copied token"
        );
        let sibling = Command::new(&runner)
            .args(["--exact", "successor_sibling_peer", "--nocapture"])
            .env("AGE319_SUCCESSOR_SIBLING", "1")
            .env("AGE319_SUCCESSOR_FACK", "1")
            .env("AGE319_SUCCESSOR_SOCKET", &socket)
            .env("AGE319_SUCCESSOR_ALLOCATION", &allocation)
            .env("AGE319_SUCCESSOR_SOURCE", &source)
            .env("AGE319_SUCCESSOR_SEQ", seq.to_string())
            .env("AGE319_SUCCESSOR_REQUEST", &request)
            .env("AGE319_SUCCESSOR_DELIVERY_REQUEST", delivery_request_id)
            .env("AGE319_SUCCESSOR_TOKEN", token)
            .status()
            .unwrap();
        assert!(sibling.success(), "sibling gained copied F/token authority");
        let receipt_path = broker_root.join("v30/successor-receipts").join(format!(
            "{}.json",
            f["delivery"]["grant"]["grant_id"].as_str().unwrap()
        ));
        assert!(
            !receipt_path.exists(),
            "Broker manufactured receiver receipt"
        );
        assert_eq!(f["delivery"]["grant"]["source_id"], source);
        assert_eq!(f["delivery"]["grant"]["attempt_id"], attempt);
        fs::write(temp.path().join("f-check"), b"go").unwrap();
        while !temp.path().join("fack.json").exists() {
            assert!(
                Instant::now() < deadline + Duration::from_secs(15),
                "successor ACK absent"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let fack: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("fack.json")).unwrap()).unwrap();
        assert_eq!(
            fs::read(temp.path().join("received.bin")).unwrap(),
            payload,
            "F reply bytes differ from retained source"
        );
        assert_eq!(fack["ack"]["grant"]["phase"], "acked");
        assert_eq!(fack["ack"]["generation"], offer["generation"]);
        let side = Connection::open(&sidecar_path).unwrap();
        let (attempts, receipt_sha, ack_sha): (i64, String, String) = side
            .query_row(
                "SELECT m.delivery_attempts,c.receipt_sha256,e.receipt_sha256
             FROM fresh_successor_grant g
             JOIN fresh_successor_receipt c ON c.grant_id=g.grant_id
             JOIN fresh_successor_ack_evidence e ON e.grant_id=g.grant_id
             JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
             JOIN fresh_successor_admission a ON a.generation=g.generation
             JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
             WHERE g.delivery_request_id=?1 AND g.generation=?2
               AND a.offer_request_id=g.offer_request_id
               AND a.session_id=g.session_id AND a.seq=g.seq
               AND a.source_id=g.source_id AND a.attempt_id=g.attempt_id
               AND a.successor_identity=g.successor_identity
               AND a.payload_sha256=g.payload_sha256 AND a.payload_byte_len=g.payload_byte_len
               AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
               AND r.payload_sha256=g.payload_sha256 AND r.payload_byte_len=g.payload_byte_len
               AND e.acknowledged_at=m.delivered_at
               AND m.delivered_by_invocation_uuid=g.grant_id",
                params![delivery_request_id, offer["generation"].as_str().unwrap()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(attempts, 1);
        assert_eq!(receipt_sha, ack_sha);
        assert_eq!(receipt_sha, fack["receipt_sha256"]);
        assert_eq!(
            fs::symlink_metadata(&receipt_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(
            side.query_row("SELECT count(*) FROM fresh_recipient_grant", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        broker.kill().unwrap();
        broker.wait().unwrap();
        let mut broker = start_broker(&broker_root, &socket, &runner);
        fs::write(temp.path().join("restart-ack"), b"go").unwrap();
        while !temp.path().join("restart-ack.json").exists() {
            assert!(
                Instant::now() < deadline + Duration::from_secs(30),
                "successor ACK restart readback absent"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let restart: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("restart-ack.json")).unwrap())
                .unwrap();
        assert_eq!(restart["ack"], fack["ack"]);
        assert!(peer.wait().unwrap().success());
        broker.kill().unwrap();
        broker.wait().unwrap();
        return;
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
