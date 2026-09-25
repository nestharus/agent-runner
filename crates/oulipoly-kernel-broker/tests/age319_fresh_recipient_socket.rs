#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use base64::Engine as _;
use oulipoly_kernel_broker::identity::PinnedProcess;
use oulipoly_kernel_broker::protocol::{FreshRecipientRequest, fresh_recipient_request_at};
use oulipoly_runtime::executor::cli::pty_broker::PtyControlGenerationIdentity;
use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{
    BindRuntimeGenerationRunning, BrokerSidecar, CreateRuntimeGeneration,
    FreshNativeFPrepareRequest, FreshRecipientIdentity, FreshV30Lane, MailboxDb,
    RuntimeGenerationFence, RuntimeGenerationId,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::{Child, Command};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[test]
fn private_fresh_recipient_delivery_ack_collision_and_restart() {
    if std::env::var_os("AGE319_FRESH_RECIPIENT_CHILD").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("private_fresh_recipient_delivery_ack_collision_and_restart")
            .arg("--nocapture")
            .env("AGE319_FRESH_RECIPIENT_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success(), "private recipient fixture failed");
        return;
    }
    let private = tempfile::tempdir().unwrap();
    let old_root = private.path().join("old");
    let broker_root = private.path().join("broker");
    fs::create_dir(&old_root).unwrap();
    fs::create_dir(&broker_root).unwrap();
    fs::set_permissions(&broker_root, fs::Permissions::from_mode(0o700)).unwrap();
    drop(StateDb::open(&old_root.join("state.db")).unwrap());
    drop(MailboxDb::open(&old_root.join("pid-identity.db")).unwrap());
    let old = Connection::open(old_root.join("pid-identity.db")).unwrap();
    let lane_id = FreshV30Lane::initialize_at(&broker_root).unwrap();
    let mut lane = FreshV30Lane::open_at(&broker_root).unwrap();
    let allocation = uuid::Uuid::new_v4().to_string();
    let session = lane.allocate_session(&allocation).unwrap();
    let state = Connection::open(broker_root.join("v30/state.db")).unwrap();
    let sidecar_path = broker_root.join("v30/sidecar/pid-identity.db");
    let fresh = Connection::open(&sidecar_path).unwrap();
    let owner = identity(std::process::id() as i32);
    let root_id = uuid::Uuid::new_v4().to_string();
    let owner_generation = uuid::Uuid::new_v4().to_string();
    let source_id = uuid::Uuid::new_v4().to_string();
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let admission_id = uuid::Uuid::new_v4().to_string();
    let digest = "a".repeat(64);
    state.execute(
        "INSERT INTO fresh_lane_accepted_source VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'2026-09-24T00:00:00Z')",
        params![source_id, attempt_id, admission_id, digest, lane_id.source_generation,
            lane_id.lane_id, root_id, owner_generation],
    ).unwrap();
    fresh
        .execute(
            "INSERT INTO fresh_recipient_source VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                source_id,
                attempt_id,
                admission_id,
                digest,
                lane_id.source_generation,
                lane_id.lane_id,
                root_id,
                owner_generation
            ],
        )
        .unwrap();
    fresh
        .execute(
            "INSERT INTO fresh_recipient_binding VALUES(?1,?2,?3,?4,?5)",
            params![
                session.session_id,
                serde_json::to_string(&owner).unwrap(),
                root_id,
                owner_generation,
                lane_id.source_generation
            ],
        )
        .unwrap();
    attach_state_recipient(
        &state,
        &session.session_id,
        &lane_id.lane_id,
        &lane_id.source_generation,
        &root_id,
        &owner_generation,
        &owner,
    );
    let payloads = [
        b"exact first payload".as_slice(),
        b"second".as_slice(),
        b"intervening".as_slice(),
        b"fourth".as_slice(),
    ];
    let mut seqs = Vec::new();
    for (index, payload) in payloads.iter().enumerate() {
        let (seq, sha, len) = insert_fresh_payload(
            &fresh,
            &sidecar_path,
            &session.session_id,
            &format!("fresh-{index}"),
            payload,
        );
        fresh
            .execute(
                "INSERT INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
                params![session.session_id, seq, source_id, attempt_id, sha, len],
            )
            .unwrap();
        seqs.push(seq);
    }
    // Exact same session and sequence in v29 is independent pending debt.
    old.execute(
        "INSERT INTO mailbox(seq,session_id,kind,handle,payload_json,enqueued_at,
          state_dir,meta_path,log_path,rc_path,rc)
         VALUES(?1,?2,'fixture','old-collision','{}','2026-09-24T00:00:00Z',
          '/old','/old/meta','/old/log','/old/rc',0)",
        params![seqs[0], session.session_id],
    )
    .unwrap();
    assert!(
        lane.lookup_payload("", &session.session_id, seqs[0])
            .is_err()
    );
    drop(lane);

    let socket = private.path().join("v30.sock");
    let runner = std::env::current_exe().unwrap();
    let mut broker = start_broker(&broker_root, &socket, &runner);
    let first_request = uuid::Uuid::new_v4().to_string();
    drop_reply(
        &socket,
        &FreshRecipientRequest::Submit {
            allocation_request_id: allocation.clone(),
            delivery_request_id: first_request.clone(),
        },
    );
    let first = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Read {
            delivery_request_id: first_request.clone(),
        },
    )
    .unwrap();
    let first_grant = &first["grant"];
    assert_eq!(first_grant["seq"].as_i64(), Some(seqs[0]));
    assert!(matches!(
        first_grant["phase"].as_str(),
        Some("unknown" | "submitted")
    ));
    assert!(first_grant.get("delivery_token").is_none());
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Submit {
                allocation_request_id: allocation.clone(),
                delivery_request_id: first_request.clone(),
            }
        )
        .is_err()
    );
    let first_id = first_grant["grant_id"].as_str().unwrap().to_string();
    let first_recovery = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Recover {
            delivery_request_id: first_request.clone(),
        },
    )
    .unwrap();
    let first_token = first_recovery["grant"]["delivery_token"]
        .as_str()
        .unwrap()
        .to_string();
    let generation_id = RuntimeGenerationId::new();
    let control = private.path().join("native-f-control.sock");
    let invocation = uuid::Uuid::new_v4().to_string();
    let mut runtime =
        BrokerSidecar::open_existing(&sidecar_path, &broker_root.join("v30")).unwrap();
    runtime
        .mailbox_mut()
        .runtime_lifecycle()
        .create_runtime_generation(CreateRuntimeGeneration {
            generation_id: &generation_id,
            spawn_invocation_uuid: &invocation,
            session_id: Some(&session.session_id),
            runtime_mode: "pty_interactive",
            provider_name: "fixture-provider",
            model_name: None,
            pty_control_path: control.to_str(),
            models_dir: None,
            effective_cwd: None,
        })
        .unwrap();
    let live =
        oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
            .unwrap()
            .unwrap();
    runtime
        .mailbox_mut()
        .runtime_lifecycle()
        .bind_runtime_generation_running(BindRuntimeGenerationRunning {
            fence: RuntimeGenerationFence {
                generation_id: &generation_id,
                spawn_invocation_uuid: &invocation,
            },
            spawned_os_pid: live.os_pid,
            exact_process_identity: &live,
            os_pgid: None,
        })
        .unwrap();
    drop(runtime);
    let preparation = FreshNativeFPrepareRequest {
        preparation_request_id: uuid::Uuid::new_v4().to_string(),
        delivery_request_id: first_request.clone(),
        grant_id: first_id.clone(),
        delivery_token: first_token.clone(),
        runtime_generation_id: generation_id.to_string(),
        provider_instance_id: "fixture-instance".into(),
        settings_id: "fixture-settings".into(),
        envelope_nonce: uuid::Uuid::new_v4().to_string(),
        tail_resume_token: "typed-tail-token".into(),
    };
    let prepare = |input: FreshNativeFPrepareRequest| {
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::PrepareNativeF { preparation: input },
        )
    };
    let mut bad = preparation.clone();
    bad.tail_resume_token.clear();
    assert!(prepare(bad).is_err(), "missing Tail anchor must refuse");
    let mut bad = preparation.clone();
    bad.runtime_generation_id = uuid::Uuid::new_v4().to_string();
    assert!(prepare(bad).is_err(), "wrong generation must refuse");
    let mut bad = preparation.clone();
    bad.delivery_token = uuid::Uuid::new_v4().to_string();
    assert!(prepare(bad).is_err(), "wrong F token must refuse");
    assert!(
        prepare(preparation.clone()).is_err(),
        "absent PTY socket must refuse"
    );
    let mut control_server = ControlFixtureServer::start(
        &control,
        &generation_id.to_string(),
        &invocation,
        &session.session_id,
        &live,
    );
    for change in [
        "generation",
        "provider",
        "instance",
        "settings",
        "session",
        "peer",
        "child",
    ] {
        control_server.change(change);
        assert!(
            prepare(preparation.clone()).is_err(),
            "{change} identity must refuse"
        );
    }
    control_server.restore();
    control_server.drop_next_reply();
    assert!(
        prepare(preparation.clone()).is_err(),
        "lost identity query reply must refuse"
    );
    assert_eq!(
        fresh
            .query_row("SELECT count(*) FROM fresh_native_f_preparation", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        fresh
            .query_row("SELECT count(*) FROM fresh_recipient_grant", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        fresh
            .query_row(
                "SELECT count(*) FROM fresh_recipient_ack_evidence",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop_reply(
        &socket,
        &FreshRecipientRequest::PrepareNativeF {
            preparation: preparation.clone(),
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let prepared = loop {
        let read = fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::ReadNativeFPreparation {
                preparation_request_id: preparation.preparation_request_id.clone(),
            },
        )
        .unwrap();
        if !read["preparation"].is_null() {
            break read;
        }
        assert!(
            Instant::now() < deadline,
            "lost preparation reply had no durable readback"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(prepared["kind"], "native_f_preparation_readback");
    let record = &prepared["preparation"];
    assert_eq!(record["grant_id"], first_id);
    assert_eq!(record["session_id"], session.session_id);
    assert_eq!(record["source_id"], source_id);
    assert_eq!(
        record["payload_sha256"],
        format!("{:x}", Sha256::digest(payloads[0]))
    );
    assert_eq!(record["runtime_generation_id"], generation_id.to_string());
    assert_eq!(record["tail_resume_token"], "typed-tail-token");
    let envelope = record["envelope_text"].as_str().unwrap();
    assert!(envelope.contains(&preparation.envelope_nonce));
    assert_eq!(
        record["envelope_sha256"],
        format!("{:x}", Sha256::digest(envelope.as_bytes()))
    );
    let read_prepared = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::ReadNativeFPreparation {
            preparation_request_id: preparation.preparation_request_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&read_prepared["preparation"]).unwrap(),
        serde_json::to_vec(record).unwrap()
    );
    assert_eq!(
        prepare(preparation.clone()).unwrap()["preparation"],
        record.clone()
    );
    let mut bad = preparation.clone();
    bad.tail_resume_token = "changed-tail".into();
    assert!(prepare(bad).is_err(), "changed Tail anchor must refuse");
    let mut bad = preparation.clone();
    bad.provider_instance_id = "different-instance".into();
    assert!(prepare(bad).is_err(), "changed endpoint must refuse");
    assert_ne!(first_recovery["grant"]["phase"], "acked");
    assert!(
        fresh
            .query_row(
                "SELECT delivered_at IS NULL FROM mailbox WHERE seq=?1",
                [seqs[0]],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );
    assert_eq!(
        fresh
            .query_row(
                "SELECT count(*) FROM fresh_recipient_ack_evidence",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        Connection::open(broker_root.join("v30/state.db"))
            .unwrap()
            .query_row("SELECT count(*) FROM fresh_root_publication", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(first_recovery["payload_base64"].as_str().unwrap())
            .unwrap(),
        payloads[0]
    );
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Acknowledge {
                grant_id: first_id.clone(),
                delivery_token: uuid::Uuid::new_v4().to_string(),
            }
        )
        .is_err()
    );
    let wrong = Command::new(&runner)
        .arg("--exact")
        .arg("wrong_recipient_child")
        .env("AGE319_WRONG_RECIPIENT_CHILD", "1")
        .env("AGE319_FRESH_SOCKET", &socket)
        .env("AGE319_FRESH_ALLOCATION", &allocation)
        .env("AGE319_FRESH_REQUEST", &first_request)
        .env("AGE319_FRESH_GRANT", &first_id)
        .env("AGE319_FRESH_TOKEN", &first_token)
        .env(
            "AGE319_FRESH_PREPARATION",
            &preparation.preparation_request_id,
        )
        .env(
            "AGE319_FRESH_PREP_REQUEST",
            serde_json::to_string(&preparation).unwrap(),
        )
        .status()
        .unwrap();
    assert!(wrong.success());
    assert_eq!(
        fresh
            .query_row(
                "SELECT delivered_at IS NULL FROM mailbox WHERE seq=?1",
                [seqs[0]],
                |r| r.get::<_, bool>(0)
            )
            .unwrap(),
        true
    );
    broker.kill().unwrap();
    broker.wait().unwrap();
    broker = start_broker(&broker_root, &socket, &runner);
    let restarted_preparation = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::ReadNativeFPreparation {
            preparation_request_id: preparation.preparation_request_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&restarted_preparation["preparation"]).unwrap(),
        serde_json::to_vec(record).unwrap()
    );
    let recovered = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Read {
            delivery_request_id: first_request.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        recovered["grant"]["grant_id"].as_str(),
        Some(first_id.as_str())
    );
    assert!(recovered["grant"].get("delivery_token").is_none());
    let after_restart = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Recover {
            delivery_request_id: first_request,
        },
    )
    .unwrap();
    assert_eq!(
        after_restart["grant"]["delivery_token"].as_str(),
        Some(first_token.as_str())
    );
    let lookup = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Lookup {
            lane_id: lane_id.lane_id.clone(),
            session_id: session.session_id.clone(),
            seq: seqs[0],
        },
    )
    .unwrap();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(lookup["payload_base64"].as_str().unwrap())
            .unwrap(),
        payloads[0]
    );
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Lookup {
                lane_id: uuid::Uuid::new_v4().to_string(),
                session_id: session.session_id.clone(),
                seq: seqs[0],
            }
        )
        .is_err()
    );

    // Create three more exact grants. Only the named second and fourth may be
    // consumed by the cleanup actor; the intervening third remains pending.
    let mut next = Vec::new();
    for seq in &seqs[1..] {
        let response = fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Submit {
                allocation_request_id: allocation.clone(),
                delivery_request_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .unwrap();
        assert_eq!(response["grant"]["seq"].as_i64(), Some(*seq));
        next.push(response["grant"].clone());
    }
    let mut duplicate_nonce = preparation.clone();
    duplicate_nonce.preparation_request_id = uuid::Uuid::new_v4().to_string();
    // The grant readback intentionally omits the request ID. Recover the
    // fixture's exact second grant key from its immutable row for this check.
    duplicate_nonce.delivery_request_id = fresh
        .query_row(
            "SELECT delivery_request_id FROM fresh_recipient_grant WHERE grant_id=?1",
            [next[0]["grant_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    duplicate_nonce.grant_id = next[0]["grant_id"].as_str().unwrap().into();
    duplicate_nonce.delivery_token = next[0]["delivery_token"].as_str().unwrap().into();
    assert!(
        prepare(duplicate_nonce).is_err(),
        "same nonce with different W envelope must refuse"
    );
    fresh
        .execute(
            "INSERT INTO runtime_generation
         (generation_uuid,lifecycle_state,spawn_invocation_uuid,session_id,
          runtime_mode,provider_name,pty_control_path,created_at)
         VALUES(?1,'starting',?2,?3,'pty_interactive','other-provider',?4,
                '2026-09-24T00:00:00Z')",
            params![
                uuid::Uuid::new_v4().to_string(),
                uuid::Uuid::new_v4().to_string(),
                session.session_id,
                control.to_str().unwrap()
            ],
        )
        .unwrap();
    let mut ambiguous = preparation.clone();
    ambiguous.preparation_request_id = uuid::Uuid::new_v4().to_string();
    ambiguous.envelope_nonce = uuid::Uuid::new_v4().to_string();
    ambiguous.delivery_request_id = fresh
        .query_row(
            "SELECT delivery_request_id FROM fresh_recipient_grant WHERE grant_id=?1",
            [next[0]["grant_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    ambiguous.grant_id = next[0]["grant_id"].as_str().unwrap().into();
    ambiguous.delivery_token = next[0]["delivery_token"].as_str().unwrap().into();
    assert!(
        prepare(ambiguous).is_err(),
        "ambiguous resident generation must refuse"
    );
    control_server.stop();
    fs::remove_file(&control).unwrap();
    assert!(
        prepare(preparation.clone()).is_err(),
        "lost socket must refuse readback"
    );
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::ReadNativeFPreparation {
                preparation_request_id: preparation.preparation_request_id.clone(),
            }
        )
        .is_err(),
        "lost socket must refuse live readback"
    );
    let _replacement = UnixListener::bind(&control).unwrap();
    assert!(
        prepare(preparation.clone()).is_err(),
        "replacement inode must refuse"
    );
    let delegation_file = private.path().join("delegation");
    let result_file = private.path().join("result");
    let mut delegate = Command::new(&runner)
        .arg("--exact")
        .arg("delegate_recipient_child")
        .env("AGE319_DELEGATE_CHILD", "1")
        .env("AGE319_FRESH_SOCKET", &socket)
        .env("AGE319_DELEGATION_FILE", &delegation_file)
        .env("AGE319_DELEGATION_RESULT", &result_file)
        .spawn()
        .unwrap();
    let delegate_identity = identity(delegate.id() as i32);
    let batch = fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Delegate {
            grant_ids: vec![
                next[0]["grant_id"].as_str().unwrap().into(),
                next[2]["grant_id"].as_str().unwrap().into(),
            ],
            delegate: delegate_identity,
        },
    )
    .unwrap();
    let delegation_id = batch["batch"]["delegation_id"].as_str().unwrap();
    fs::write(&delegation_file, delegation_id).unwrap();
    assert!(delegate.wait().unwrap().success());
    assert_eq!(fs::read_to_string(&result_file).unwrap(), "delegated_ack");
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::AcknowledgeDelegated {
                delegation_id: delegation_id.into(),
            }
        )
        .is_err()
    );
    for (index, seq) in seqs.iter().enumerate() {
        let delivered: bool = fresh
            .query_row(
                "SELECT delivered_at IS NOT NULL FROM mailbox WHERE seq=?1",
                [seq],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(delivered, index == 1 || index == 3);
    }
    for grant in [&first_recovery["grant"], &next[1]] {
        let ack = fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Acknowledge {
                grant_id: grant["grant_id"].as_str().unwrap().into(),
                delivery_token: grant["delivery_token"].as_str().unwrap().into(),
            },
        )
        .unwrap();
        assert_eq!(ack["grant"]["phase"].as_str(), Some("acked"));
    }
    let evidence = fresh
        .prepare(
            "SELECT e.basis,e.delivery_token_sha256,g.delivery_token,e.session_id,e.seq,
                e.source_id,e.attempt_id,e.payload_sha256,e.payload_byte_len
         FROM fresh_recipient_ack_evidence e
         JOIN fresh_recipient_grant g ON g.grant_id=e.grant_id ORDER BY e.seq",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(evidence.len(), 4);
    for (
        index,
        (basis, token_sha, token, row_session, seq, row_source, row_attempt, row_sha, row_len),
    ) in evidence.iter().enumerate()
    {
        assert_eq!(
            basis,
            if index == 1 || index == 3 {
                "delegated_manual_ack"
            } else {
                "manual_ack"
            }
        );
        assert_eq!(
            *token_sha,
            format!("{:x}", Sha256::digest(token.as_bytes()))
        );
        assert_eq!(row_session, &session.session_id);
        assert_eq!(*seq, seqs[index]);
        assert_eq!(row_source, &source_id);
        assert_eq!(row_attempt, &attempt_id);
        assert_eq!(row_sha, &format!("{:x}", Sha256::digest(&payloads[index])));
        assert_eq!(*row_len, payloads[index].len() as i64);
    }
    assert_eq!(
        fresh
            .query_row("SELECT count(*) FROM completion_event_listener", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Acknowledge {
                grant_id: first_id,
                delivery_token: first_token,
            }
        )
        .is_err()
    );
    assert_eq!(
        old.query_row(
            "SELECT delivered_at IS NULL FROM mailbox WHERE seq=?1",
            [seqs[0]],
            |r| r.get::<_, bool>(0)
        )
        .unwrap(),
        true
    );
    drop(old);
    let mut old_route = MailboxDb::open(&old_root.join("pid-identity.db")).unwrap();
    assert_eq!(
        old_route
            .acknowledge_range(&session.session_id, seqs[0], seqs[0], "old-owner")
            .unwrap(),
        1
    );

    // Unacknowledged older grants do not impose a fixed 32-row delivery
    // cutoff on later exact pending rows.
    let mut backlog_last = 0;
    for index in 0..33 {
        let bytes = format!("backlog-{index}");
        let (seq, sha, len) = insert_fresh_payload(
            &fresh,
            &sidecar_path,
            &session.session_id,
            &format!("backlog-{index}"),
            bytes.as_bytes(),
        );
        fresh
            .execute(
                "INSERT INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
                params![session.session_id, seq, source_id, attempt_id, sha, len],
            )
            .unwrap();
        let delivery = fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Submit {
                allocation_request_id: allocation.clone(),
                delivery_request_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .unwrap();
        assert_eq!(delivery["grant"]["seq"].as_i64(), Some(seq));
        backlog_last = seq;
    }
    assert!(
        fresh
            .query_row(
                "SELECT delivered_at IS NULL FROM mailbox WHERE seq=?1",
                [backlog_last],
                |r| r.get::<_, bool>(0)
            )
            .unwrap()
    );

    // A retained row for an offline original recipient is not available to a
    // live sibling, even when its source is accepted and its bytes are sound.
    let mut offline_process = Command::new("sleep").arg("60").spawn().unwrap();
    let offline_identity = identity(offline_process.id() as i32);
    offline_process.kill().unwrap();
    offline_process.wait().unwrap();
    let mut lane = FreshV30Lane::open_at(&broker_root).unwrap();
    let offline_allocation = uuid::Uuid::new_v4().to_string();
    let offline_session = lane.allocate_session(&offline_allocation).unwrap();
    fresh
        .execute(
            "INSERT INTO fresh_recipient_binding VALUES(?1,?2,?3,?4,?5)",
            params![
                offline_session.session_id,
                serde_json::to_string(&offline_identity).unwrap(),
                root_id,
                owner_generation,
                lane_id.source_generation
            ],
        )
        .unwrap();
    attach_state_recipient(
        &state,
        &offline_session.session_id,
        &lane_id.lane_id,
        &lane_id.source_generation,
        &root_id,
        &owner_generation,
        &offline_identity,
    );
    let (offline_seq, offline_sha, offline_len) = insert_fresh_payload(
        &fresh,
        &sidecar_path,
        &offline_session.session_id,
        "offline",
        b"offline payload",
    );
    fresh
        .execute(
            "INSERT INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                offline_session.session_id,
                offline_seq,
                source_id,
                attempt_id,
                offline_sha,
                offline_len
            ],
        )
        .unwrap();
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Submit {
                allocation_request_id: offline_allocation,
                delivery_request_id: uuid::Uuid::new_v4().to_string(),
            }
        )
        .is_err()
    );

    // A copied or synthetically mapped row without the matching accepted
    // fresh State source/attempt cannot mint a token or mark delivery.
    let missing_allocation = uuid::Uuid::new_v4().to_string();
    let missing_session = lane.allocate_session(&missing_allocation).unwrap();
    fresh
        .execute(
            "INSERT INTO fresh_recipient_binding VALUES(?1,?2,?3,?4,?5)",
            params![
                missing_session.session_id,
                serde_json::to_string(&owner).unwrap(),
                root_id,
                owner_generation,
                lane_id.source_generation
            ],
        )
        .unwrap();
    attach_state_recipient(
        &state,
        &missing_session.session_id,
        &lane_id.lane_id,
        &lane_id.source_generation,
        &root_id,
        &owner_generation,
        &owner,
    );
    let missing_source = uuid::Uuid::new_v4().to_string();
    let missing_attempt = uuid::Uuid::new_v4().to_string();
    fresh
        .execute(
            "INSERT INTO fresh_recipient_source VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                missing_source,
                missing_attempt,
                admission_id,
                digest,
                lane_id.source_generation,
                lane_id.lane_id,
                root_id,
                owner_generation
            ],
        )
        .unwrap();
    let (missing_seq, missing_sha, missing_len) = insert_fresh_payload(
        &fresh,
        &sidecar_path,
        &missing_session.session_id,
        "missing-source",
        b"closed",
    );
    fresh
        .execute(
            "INSERT INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                missing_session.session_id,
                missing_seq,
                missing_source,
                missing_attempt,
                missing_sha,
                missing_len
            ],
        )
        .unwrap();
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Submit {
                allocation_request_id: missing_allocation,
                delivery_request_id: uuid::Uuid::new_v4().to_string(),
            }
        )
        .is_err()
    );
    assert_eq!(
        fresh
            .query_row(
                "SELECT count(*) FROM fresh_recipient_grant WHERE session_id=?1",
                [&missing_session.session_id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let detached_allocation = uuid::Uuid::new_v4().to_string();
    let detached_session = lane.allocate_session(&detached_allocation).unwrap();
    fresh
        .execute(
            "INSERT INTO fresh_recipient_binding VALUES(?1,?2,?3,?4,?5)",
            params![
                detached_session.session_id,
                serde_json::to_string(&owner).unwrap(),
                root_id,
                owner_generation,
                lane_id.source_generation
            ],
        )
        .unwrap();
    let (detached_seq, detached_sha, detached_len) = insert_fresh_payload(
        &fresh,
        &sidecar_path,
        &detached_session.session_id,
        "no-state-recipient",
        b"closed",
    );
    fresh
        .execute(
            "INSERT INTO fresh_recipient_row_source VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                detached_session.session_id,
                detached_seq,
                source_id,
                attempt_id,
                detached_sha,
                detached_len
            ],
        )
        .unwrap();
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Submit {
                allocation_request_id: detached_allocation,
                delivery_request_id: uuid::Uuid::new_v4().to_string(),
            }
        )
        .is_err()
    );
    assert!(
        fresh_recipient_request_at(
            &socket,
            &FreshRecipientRequest::Lookup {
                lane_id: lane_id.lane_id.clone(),
                session_id: detached_session.session_id,
                seq: detached_seq,
            }
        )
        .is_err()
    );
    drop(lane);
    broker.kill().unwrap();
    broker.wait().unwrap();
}

struct ControlFixtureServer {
    baseline: PtyControlGenerationIdentity,
    reply: Arc<Mutex<PtyControlGenerationIdentity>>,
    drop_reply: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ControlFixtureServer {
    fn start(
        path: &Path,
        generation: &str,
        invocation: &str,
        session: &str,
        process: &oulipoly_state::pid_identity::ProcessIdentity,
    ) -> Self {
        let listener = UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let baseline = PtyControlGenerationIdentity {
            challenge: String::new(),
            generation_id: generation.into(),
            spawn_invocation_uuid: invocation.into(),
            creator_process: process.clone(),
            provider_process: process.clone(),
            provider_account: "fixture-provider".into(),
            provider_instance_id: "fixture-instance".into(),
            settings_id: "fixture-settings".into(),
            provider_session_id: session.into(),
        };
        let reply = Arc::new(Mutex::new(baseline.clone()));
        let drop_reply = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let reply = Arc::clone(&reply);
            let drop_reply = Arc::clone(&drop_reply);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let Ok((mut peer, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    peer.set_read_timeout(Some(Duration::from_millis(250)))
                        .unwrap();
                    let mut header = [0u8; 12];
                    if peer.read_exact(&mut header).is_err()
                        || &header[..4] != b"OPTY"
                        || header[4] != 1
                        || header[5] != 3
                    {
                        continue;
                    }
                    let length = u32::from_be_bytes(header[8..12].try_into().unwrap()) as usize;
                    if length > 64 {
                        continue;
                    }
                    let mut challenge = vec![0u8; length];
                    if peer.read_exact(&mut challenge).is_err() {
                        continue;
                    }
                    if drop_reply.swap(false, Ordering::AcqRel) {
                        continue;
                    }
                    let mut statement = reply.lock().unwrap().clone();
                    statement.challenge = String::from_utf8(challenge).unwrap();
                    let bytes = serde_json::to_vec(&statement).unwrap();
                    let mut response = [0u8; 12];
                    response[..4].copy_from_slice(b"OPTY");
                    response[4] = 1;
                    response[8..12].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
                    peer.write_all(&response).unwrap();
                    peer.write_all(&bytes).unwrap();
                }
            })
        };
        Self {
            baseline,
            reply,
            drop_reply,
            stop,
            worker: Some(worker),
        }
    }

    fn change(&self, field: &str) {
        let mut statement = self.baseline.clone();
        match field {
            "generation" => statement.generation_id = uuid::Uuid::new_v4().to_string(),
            "provider" => statement.provider_account = "wrong-provider".into(),
            "instance" => statement.provider_instance_id = "wrong-instance".into(),
            "settings" => statement.settings_id = "wrong-settings".into(),
            "session" => statement.provider_session_id = "wrong-session".into(),
            "peer" => statement.creator_process.os_pid += 1,
            "child" => statement.provider_process.os_pid += 1,
            _ => unreachable!(),
        }
        *self.reply.lock().unwrap() = statement;
    }

    fn restore(&self) {
        *self.reply.lock().unwrap() = self.baseline.clone();
    }
    fn drop_next_reply(&self) {
        self.drop_reply.store(true, Ordering::Release);
    }
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

impl Drop for ControlFixtureServer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn wrong_recipient_child() {
    if std::env::var_os("AGE319_WRONG_RECIPIENT_CHILD").is_none() {
        return;
    }
    let socket = std::env::var("AGE319_FRESH_SOCKET").unwrap();
    let allocation = std::env::var("AGE319_FRESH_ALLOCATION").unwrap();
    let request = std::env::var("AGE319_FRESH_REQUEST").unwrap();
    let grant = std::env::var("AGE319_FRESH_GRANT").unwrap();
    let token = std::env::var("AGE319_FRESH_TOKEN").unwrap();
    let preparation = std::env::var("AGE319_FRESH_PREPARATION").unwrap();
    let forged: FreshNativeFPrepareRequest =
        serde_json::from_str(&std::env::var("AGE319_FRESH_PREP_REQUEST").unwrap()).unwrap();
    assert!(
        fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::PrepareNativeF {
                preparation: forged
            }
        )
        .is_err()
    );
    let denied = fresh_recipient_request_at(
        Path::new(&socket),
        &FreshRecipientRequest::ReadNativeFPreparation {
            preparation_request_id: preparation,
        },
    );
    assert!(denied.is_err() || denied.unwrap()["preparation"].is_null());
    assert!(
        fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::Submit {
                allocation_request_id: allocation,
                delivery_request_id: uuid::Uuid::new_v4().to_string(),
            }
        )
        .is_err()
    );
    assert!(
        fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::Recover {
                delivery_request_id: request,
            }
        )
        .is_err()
    );
    assert!(
        fresh_recipient_request_at(
            Path::new(&socket),
            &FreshRecipientRequest::Acknowledge {
                grant_id: grant,
                delivery_token: token,
            }
        )
        .is_err()
    );
}

#[test]
fn delegate_recipient_child() {
    if std::env::var_os("AGE319_DELEGATE_CHILD").is_none() {
        return;
    }
    let file = std::env::var("AGE319_DELEGATION_FILE").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !Path::new(&file).exists() {
        assert!(Instant::now() < deadline, "delegation not issued");
        std::thread::sleep(Duration::from_millis(10));
    }
    let delegation_id = fs::read_to_string(file).unwrap();
    let socket = std::env::var("AGE319_FRESH_SOCKET").unwrap();
    let answer = fresh_recipient_request_at(
        Path::new(&socket),
        &FreshRecipientRequest::AcknowledgeDelegated { delegation_id },
    )
    .unwrap();
    fs::write(
        std::env::var("AGE319_DELEGATION_RESULT").unwrap(),
        answer["kind"].as_str().unwrap(),
    )
    .unwrap();
}

fn identity(pid: i32) -> FreshRecipientIdentity {
    let pinned = PinnedProcess::open(pid).unwrap();
    FreshRecipientIdentity {
        host_pid: pinned.host_pid,
        boot_id: pinned.boot_id,
        starttime_ticks: pinned.starttime_ticks,
        pidns_dev: pinned.pidns_dev,
        pidns_ino: pinned.pidns_ino,
    }
}

fn attach_state_recipient(
    state: &Connection,
    session: &str,
    lane: &str,
    generation: &str,
    root: &str,
    owner: &str,
    recipient: &FreshRecipientIdentity,
) {
    state.execute(
        "INSERT INTO fresh_lane_recipient_attachment VALUES(?1,?2,?3,?4,?5,?6,'2026-09-24T00:00:00Z')",
        params![session, lane, generation, root, owner, serde_json::to_string(recipient).unwrap()],
    ).unwrap();
}

fn insert_fresh_payload(
    conn: &Connection,
    sidecar: &Path,
    session: &str,
    handle: &str,
    bytes: &[u8],
) -> (i64, String, i64) {
    let sha = format!("{:x}", Sha256::digest(bytes));
    let path = sidecar
        .parent()
        .unwrap()
        .join("inbox-payloads/v1/sha256")
        .join(&sha[..2])
        .join(&sha);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    fs::File::open(&path).unwrap().sync_all().unwrap();
    conn.execute(
        "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
         state_dir,meta_path,log_path,rc_path,rc,payload_file_path,payload_sha256,
         payload_byte_len,payload_retention_policy)
         VALUES(?1,'fixture',?2,'{}','2026-09-24T00:00:00Z',
         '/fresh','/fresh/meta','/fresh/log','/fresh/rc',0,?3,?4,?5,
         'until_terminal_disposition')",
        params![
            session,
            handle,
            path.to_str().unwrap(),
            sha,
            bytes.len() as i64
        ],
    )
    .unwrap();
    (conn.last_insert_rowid(), sha, bytes.len() as i64)
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
            panic!("broker exited: {status}");
        }
        assert!(Instant::now() < deadline, "fresh socket not ready");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn drop_reply(socket: &Path, request: &FreshRecipientRequest) {
    let mut stream = UnixStream::connect(socket).unwrap();
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).unwrap();
    let mut frame = vec![b'F'];
    frame.extend_from_slice(&challenge);
    frame.extend_from_slice(&serde_json::to_vec(request).unwrap());
    stream.write_all(&frame).unwrap();
}
