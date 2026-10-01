//! Socket tests use a disposable namespace only to own synthetic State as UID
//! zero. The first case leaves the Broker fixture predicate false: namespace
//! membership alone must neither close nor authorize a bound F request.
use super::*;
use oulipoly_kernel_broker::protocol;
use oulipoly_state::mailbox::FreshRootWorkIntent;
use rusqlite::{Connection, params};
use std::process::Command;

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[test]
fn nonfixture_socket_reads_bound_terminal_and_refuses_unbound_evidence() {
    socket_case(false);
}

#[test]
fn fixture_socket_does_not_authorize_unbound_recipient_evidence() {
    socket_case(true);
}

fn socket_case(fixture: bool) {
    if std::env::var_os("AGE319_RECIPIENT_GATE_TEST_CHILD").is_none() {
        let node = if fixture {
            "linux_main::recipient_effect_tests::fixture_socket_does_not_authorize_unbound_recipient_evidence"
        } else {
            "linux_main::recipient_effect_tests::nonfixture_socket_reads_bound_terminal_and_refuses_unbound_evidence"
        };
        let status = Command::new("unshare")
            .arg("-Ur")
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", node, "--nocapture"])
            .env("AGE319_RECIPIENT_GATE_TEST_CHILD", "1")
            .env_remove("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
            .status()
            .unwrap();
        assert!(status.success(), "recipient socket child: {status}");
        return;
    }
    let scratch = std::env::temp_dir();
    let writer = scratch.parent().unwrap().parent().unwrap();
    std::env::set_current_dir(writer).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let identity = FreshV30Lane::initialize_at(temp.path()).unwrap();
    let mut lane = FreshV30Lane::open_at(temp.path()).unwrap();
    let pinned = PinnedProcess::open(std::process::id() as i32).unwrap();
    let actor = FreshRecipientIdentity {
        host_pid: pinned.host_pid,
        boot_id: pinned.boot_id.clone(),
        starttime_ticks: pinned.starttime_ticks,
        pidns_dev: pinned.pidns_dev,
        pidns_ino: pinned.pidns_ino,
    };
    // These are test-owned State seeds, not a physical release certificate.
    let stamp = PreparedProcessStamp {
        host_pid: actor.host_pid,
        boot_id: actor.boot_id.clone(),
        starttime_ticks: actor.starttime_ticks,
        pidns_dev: actor.pidns_dev,
        pidns_ino: actor.pidns_ino,
    };
    let old_owner = serde_json::from_value(serde_json::json!({
        "protocol":"completion-domain-owner-v1", "domain_id":uuid(),
        "supervisor_authority_id":uuid(), "owner_generation":uuid(),
        "guardian_identity":{"pid":actor.host_pid,"boot_id":actor.boot_id,"starttime_ticks":actor.starttime_ticks},
        "driver_identity":{"pid":actor.host_pid,"boot_id":actor.boot_id,"starttime_ticks":actor.starttime_ticks},
        "endpoint":"test-owned"
    })).unwrap();
    let root = FreshReleasedHandoff {
        handoff_id: uuid(),
        d_key: uuid(),
        invocation_uuid: uuid(),
        root_work_intent: FreshRootWorkIntent::NormalCli(vec![
            "--model".into(),
            "test-model".into(),
            "test-prompt".into(),
        ]),
        broker_incarnation: uuid(),
        runner_image_device: 1,
        runner_image_inode: 1,
        old_release: BrokerReleaseEvidence {
            prepared: PreparedBrokerOwner {
                source_generation: uuid(),
                root_id: uuid(),
                owner_uid: 1000,
                domain_id: uuid(),
                supervisor_authority_id: uuid(),
                owner_generation: uuid(),
                endpoint: "test-owned".into(),
                entry: stamp.clone(),
                guardian: stamp.clone(),
                driver: stamp.clone(),
                root_init: stamp.clone(),
                joined_child: stamp,
            },
            release_id: uuid(),
            owner: old_owner,
        },
        fresh_lane: identity,
        registration_authority: oulipoly_state::CompletionRegistrationAuthority::generate()
            .unwrap()
            .process_environment_value()
            .into(),
        delegated_h_listener_policy: None,
        delegated_root_work_authority: None,
    };
    lane.bind_released_handoff(&root, &actor).unwrap();
    let session = lane.allocate_session(&root.d_key).unwrap();
    lane.ensure_released_invocation(&root, &actor, &session)
        .unwrap();

    // Short relative socket names stay physically beneath the writer even
    // when its absolute checkout path exceeds Unix socket pathname limits.
    let socket = PathBuf::from(format!("target/tmp/recipient-{}.sock", std::process::id()));
    if fixture {
        unsafe { std::env::set_var("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket) };
    }
    assert_eq!(private_fixture(), fixture);
    println!(
        "actual private_fixture={fixture}; synthetic State={}",
        temp.path().display()
    );
    let server_state = temp.path().to_path_buf();
    let server_socket = socket.clone();
    let image = File::open(std::env::current_exe().unwrap()).unwrap();
    std::thread::spawn(move || {
        serve_fresh_v30_at(
            &server_state,
            &server_socket,
            image,
            None,
            None,
            None,
            None,
            None,
            Arc::new(super::AdmissionFences::new(HashSet::new())),
        )
        .unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !socket.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "test socket unavailable"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let terminal = protocol::fresh_root_terminal_request_at(
        &socket,
        &FreshRecipientRequest::ReadRootTerminal {
            d_key: root.d_key.clone(),
        },
    )
    .unwrap();
    assert_eq!(terminal.d_key, root.d_key);
    assert_eq!(terminal.session_id, session.session_id);
    assert_eq!(terminal.execution_state, "unknown"); // No K/Q fabricated.
    println!("bound terminal returned; execution remains unknown");

    for (label, d_key) in [
        ("unbound", uuid()),
        ("private-prefix", "private:old-session".into()),
    ] {
        let error = protocol::fresh_root_terminal_request_at(
            &socket,
            &FreshRecipientRequest::ReadRootTerminal { d_key },
        )
        .unwrap_err();
        println!("{label} denied: {error}");
    }
    let sidecar = Connection::open(temp.path().join("v30/sidecar/pid-identity.db")).unwrap();
    let copied_request = uuid();
    let copied_grant = uuid();
    let old_payload = b"copied v29 payload";
    let old_sha = format!("{:x}", Sha256::digest(old_payload));
    let old_path = temp.path().join("copied-old-payload");
    fs::write(&old_path, old_payload).unwrap();
    sidecar
        .execute(
            "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
         state_dir,meta_path,log_path,rc_path,rc,payload_file_path,payload_sha256,
         payload_byte_len,payload_retention_policy)
         VALUES(?1,'agent_bash_complete','old-handle','{}','old-created',
         '/old','/old/meta','/old/log','/old/rc',0,?2,?3,?4,'until_terminal_disposition')",
            params![
                session.session_id,
                old_path.to_str().unwrap(),
                old_sha,
                old_payload.len() as i64
            ],
        )
        .unwrap();
    let old_seq = sidecar.last_insert_rowid();
    sidecar.execute("INSERT INTO fresh_recipient_grant VALUES(?1,?2,?3,?4,?13,?5,?6,?7,?8,?9,?10,?11,?12,?14,'submitted','old-created','old-submitted',NULL)",
        params![copied_grant,copied_request,uuid(),session.session_id,uuid(),uuid(),
            session.lane_id,session.source_generation,uuid(),uuid(),
            serde_json::to_string(&actor).unwrap(),old_sha,old_seq,old_payload.len() as i64]).unwrap();
    let error = protocol::fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Read {
            delivery_request_id: copied_request,
        },
    )
    .unwrap_err();
    println!("copied old grant/mailbox lineage denied: {error}");
    // Even the correct D/J session cannot authorize F without C/W/attachment.
    let error = protocol::fresh_recipient_request_at(
        &socket,
        &FreshRecipientRequest::Submit {
            allocation_request_id: root.d_key.clone(),
            delivery_request_id: uuid(),
        },
    )
    .unwrap_err();
    println!("missing recipient attachment/W denied: {error}");
    fs::remove_file(socket).unwrap();
}
