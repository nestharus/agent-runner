//! Source-only private user-namespace exercise of the actual broker launch and
//! opt-in Runner entry. It never exercises installed host-root sudo authority.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
use oulipoly_kernel_broker::protocol::{
    self, AcceptedWorkSpec, JoinSpec, Operation, ProcessWitness, SourceScope, SourceSocketWitness,
};
use oulipoly_state::mailbox::{BrokerSidecar, MailboxDb};
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn eventually(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(
            Instant::now() < until,
            "private root join fixture timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn inner() {
    let mode = std::env::var("AGE319_PRIVATE_JOIN_MODE").unwrap_or_else(|_| "help".into());
    let runner =
        std::env::var("OULIPOLY_AGE319_RUNNER_IMAGE").expect("built Runner image required");
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("data");
    let broker_state = temp.path().join("broker-state");
    let gate = temp.path().join("gate");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&broker_state).unwrap();
    fs::create_dir(&gate).unwrap();
    let mailbox =
        MailboxDb::open_completion_continuation_domain(&data.join("pid-identity.db")).unwrap();
    let domain = mailbox.completion_continuation_domain().unwrap().unwrap();
    drop(mailbox);
    drop(oulipoly_state::StateDb::open(&data.join("state.db")).unwrap());
    let broker_generation = if mode == "broker_state" {
        let sidecar_dir = broker_state.join("sidecar");
        fs::create_dir(&sidecar_dir).unwrap();
        fs::set_permissions(&sidecar_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let target = sidecar_dir.join("pid-identity.db");
        rusqlite::Connection::open(data.join("pid-identity.db"))
            .unwrap()
            .execute("VACUUM INTO ?1", [target.to_str().unwrap()])
            .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        Some(BrokerSidecar::activate_quiesced_copy(&target, &broker_state).unwrap())
    } else {
        None
    };
    let socket = temp.path().join("broker.sock");
    let broker_log = temp.path().join("broker.log");
    let mut broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&broker_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| socket.exists() || broker.try_wait().unwrap().is_some());
    assert!(
        socket.exists(),
        "broker startup: {}",
        fs::read_to_string(&broker_log).unwrap()
    );
    if let Some(generation) = broker_generation {
        // The test executable shares UID and broker access but is not the
        // fixed Runner image. It cannot inspect the route or select a path.
        assert!(protocol::state_route_at(&socket).is_err());
        // The user-owned source is now stale and even invalid. The Runner's
        // first production preflight must still learn v30 from the live
        // broker, before parsing any copied row or reserving an entry.
        fs::write(data.join("pid-identity.db"), b"retired copied sidecar").unwrap();
        let invoke = || {
            Command::new(&runner)
                .arg("--help")
                .env("OULIPOLY_DATA_DIR", &data)
                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env_remove("LD_LIBRARY_PATH")
                .output()
                .unwrap()
        };
        let first = invoke();
        assert!(!first.status.success());
        assert!(
            String::from_utf8_lossy(&first.stderr).contains(&format!(
                "broker-owned State generation {generation} requires production client routing"
            )),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        stop(&mut broker);
        let unavailable = invoke();
        assert!(!unavailable.status.success());
        assert!(
            String::from_utf8_lossy(&unavailable.stderr).contains("broker State route unavailable")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        let restart_log = temp.path().join("state-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| {
            restarted.try_wait().unwrap().is_some()
                || protocol::request_at(&socket, Operation::Classify).is_ok()
        });
        assert!(restarted.try_wait().unwrap().is_none());
        let second = invoke();
        assert!(!second.status.success());
        assert!(String::from_utf8_lossy(&second.stderr).contains(&generation));
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        stop(&mut restarted);
        return;
    }
    let unsupported = Command::new(&runner)
        .args(["--model", "age319-missing-model", "hello"])
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("unsupported kernel CLI mode"));
    assert_eq!(
        fs::read_dir(broker_state.join("entries")).unwrap().count(),
        0
    );
    let out = temp.path().join("runner.out");
    let err = temp.path().join("runner.err");
    let args: &[&str] = if mode == "diagnostics" {
        &["diagnostics", "metrics", "--minutes", "1", "--json"]
    } else if mode == "join_only" {
        &["__age319-private-join-only-v1"]
    } else {
        &["--help"]
    };
    let mut entry = Command::new(&runner)
        .args(args)
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::from(File::create(&out).unwrap()))
        .stderr(Stdio::from(File::create(&err).unwrap()))
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while !gate.join("ready").exists() && entry.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < until,
            "entry stalled: {} / {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        gate.join("ready").exists(),
        "entry failed: {} / {}",
        fs::read_to_string(&err).unwrap(),
        fs::read_to_string(&broker_log).unwrap()
    );
    // The exact Runner is still blocked before exec, although the namespace
    // and durable one-use entry transition already exist.
    assert!(entry.try_wait().unwrap().is_none());
    assert_eq!(fs::metadata(&out).unwrap().len(), 0);
    let records: Vec<_> = fs::read_dir(broker_state.join("entries"))
        .unwrap()
        .collect();
    assert_eq!(records.len(), 1);
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap()).unwrap();
    assert_eq!(record["domain_id"], domain);
    assert_eq!(record["join_consumed"], true);
    let root = record["root_id"].as_str().unwrap();
    let root_record: serde_json::Value =
        serde_json::from_slice(&fs::read(broker_state.join(format!("{root}.json"))).unwrap())
            .unwrap();
    let init_pid = root_record["init_host_pid"].as_i64().unwrap() as i32;
    let guardian_pid = record["guardian"]["host_pid"].as_i64().unwrap() as i32;
    let mailbox = MailboxDb::open(&data.join("pid-identity.db")).unwrap();
    let owner = mailbox.completion_continuation_owner().unwrap().unwrap();
    assert_eq!(owner.guardian_identity.pid, i64::from(guardian_pid));
    assert_eq!(owner.domain_id, domain);
    assert_eq!(
        owner.supervisor_authority_id,
        record["supervisor_authority_id"].as_str().unwrap()
    );
    assert_eq!(
        mailbox
            .completion_owner_kernel_root_id(&owner.owner_generation)
            .unwrap()
            .as_deref(),
        Some(root)
    );
    assert_eq!(
        fs::read_link(format!("/proc/{guardian_pid}/ns/pid")).unwrap(),
        fs::read_link("/proc/self/ns/pid").unwrap()
    );
    let child_pid: i32 = fs::read_to_string(gate.join("ready"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(record["joined_child"]["host_pid"], child_pid);
    assert!(record["joined_child"]["starttime_ticks"].as_u64().is_some());
    assert_ne!(init_pid, child_pid);
    assert_eq!(
        fs::read_link(format!("/proc/{init_pid}/ns/pid")).unwrap(),
        fs::read_link(format!("/proc/{child_pid}/ns/pid")).unwrap()
    );
    assert_ne!(
        fs::read_link(format!("/proc/{init_pid}/ns/pid")).unwrap(),
        fs::read_link("/proc/self/ns/pid").unwrap()
    );
    fs::write(gate.join("release"), b"yes").unwrap();
    eventually(|| entry.try_wait().unwrap().is_some());
    let exit = entry.wait().unwrap();
    let entry_error = fs::read_to_string(&err).unwrap();
    assert!(
        exit.success(),
        "entry: {entry_error} broker: {}",
        fs::read_to_string(&broker_log).unwrap()
    );
    let output = fs::read_to_string(&out).unwrap();
    if mode == "diagnostics" {
        assert!(output.trim_start().starts_with('{'), "{output}");
    } else if mode == "join_only" {
        assert!(output.is_empty(), "{output}");
    } else {
        assert!(output.contains("Usage:"));
    }
    let sibling = Command::new(&runner)
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!sibling.status.success());
    assert!(String::from_utf8_lossy(&sibling.stderr).contains("reservation refused"));
    // Possession of the root/owner IDs and writable acceptance-shaped files
    // does not let a sibling manufacture a positive guardian grant.
    let forged_state = temp.path().join("forged-work");
    fs::create_dir(&forged_state).unwrap();
    let forged_intent = forged_state.join("root-work-intent-v1.json");
    let forged_acceptance = forged_state.join("root-work-accepted-v1.json");
    fs::write(&forged_intent, b"{}").unwrap();
    fs::write(&forged_acceptance, b"{}").unwrap();
    let image = File::open(&runner).unwrap();
    let intent = File::open(&forged_intent).unwrap();
    let accepted = File::open(&forged_acceptance).unwrap();
    let state = File::open(&forged_state).unwrap();
    let cwd = File::open(".").unwrap();
    let false_grant = protocol::prepare_accepted_work_at(
        &socket,
        &AcceptedWorkSpec {
            root_id: root.into(),
            work_id: "forged-work".into(),
            request_sha256: "0".repeat(64),
            accepted_sha256: "0".repeat(64),
            owner_generation: owner.owner_generation.clone(),
        },
        [
            image.as_raw_fd(),
            intent.as_raw_fd(),
            cwd.as_raw_fd(),
            state.as_raw_fd(),
            accepted.as_raw_fd(),
        ],
    )
    .unwrap();
    assert!(false_grant.starts_with("error "), "{false_grant}");
    assert_eq!(
        fs::read_dir(broker_state.join("grants")).unwrap().count(),
        0
    );
    // The caller is a sibling executable, not the pinned entry. The root ID
    // and complete alleged binding do not turn it into launch authority.
    let cwd = File::open(".").unwrap();
    let (_receipt, completion) = UnixStream::pair().unwrap();
    let spec = JoinSpec {
        root_id: root.into(),
        domain_id: domain,
        supervisor_id: record["supervisor_authority_id"].as_str().unwrap().into(),
        guardian_pid: record["guardian"]["host_pid"].as_i64().unwrap() as i32,
        root_authority: "{}".into(),
        args: vec!["--help".into()],
        environment: vec![],
    };
    let replay = protocol::join_at(
        &socket,
        &spec,
        [0, 1, 2, cwd.as_raw_fd(), completion.as_raw_fd()],
    )
    .unwrap();
    assert!(replay.starts_with("error "), "{replay}");
    assert!(
        protocol::request_at(&socket, Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    stop(&mut broker);
    // Restart reads the same root PID1 and spent join record. An uncertain
    // response cannot fork another original entry.
    assert!(Path::new(&format!("/proc/{init_pid}")).exists());
    let restart_log = temp.path().join("restart.log");
    let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        protocol::request_at(&socket, Operation::Classify).is_ok()
            || restarted.try_wait().unwrap().is_some()
    });
    assert!(
        restarted.try_wait().unwrap().is_none(),
        "restart: {}",
        fs::read_to_string(&restart_log).unwrap()
    );
    assert!(
        protocol::request_at(&socket, Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    let (wrong_socket, _other_end) = UnixStream::pair().unwrap();
    let replayed = SourceSocketWitness {
        root_id: root.into(),
        domain_id: owner.domain_id.clone(),
        supervisor_id: owner.supervisor_authority_id.clone(),
        guardian: ProcessWitness {
            host_pid: guardian_pid,
            boot_id: owner.guardian_identity.boot_id.clone(),
            starttime_ticks: u64::try_from(owner.guardian_identity.starttime_ticks).unwrap(),
        },
        source: ProcessWitness {
            host_pid: child_pid,
            boot_id: record["joined_child"]["boot_id"].as_str().unwrap().into(),
            starttime_ticks: record["joined_child"]["starttime_ticks"].as_u64().unwrap(),
        },
        scope: SourceScope::Root,
    };
    // Restart cannot turn the dead joined child's old host identity into the
    // current caller, even while its root PID1 and grant record remain live.
    assert!(
        protocol::verify_source_socket_at(&socket, &replayed, wrong_socket.as_raw_fd()).is_err()
    );
    stop(&mut restarted);
    assert!(
        protocol::verify_source_socket_at(&socket, &replayed, wrong_socket.as_raw_fd()).is_err()
    );
    // Explicit fixture teardown does not claim a production drain or safe
    // registry retirement protocol.
    unsafe {
        libc::kill(init_pid, libc::SIGKILL);
    }
}

#[test]
fn original_runner_joins_once_behind_persistent_root_pid1() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    if std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE").is_none() {
        return;
    }
    for mode in ["help", "diagnostics", "join_only", "broker_state"] {
        let output = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "original_runner_joins_once_behind_persistent_root_pid1",
                "--nocapture",
            ])
            .env("AGE319_PRIVATE_JOIN_INNER", "1")
            .env("AGE319_PRIVATE_JOIN_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}
