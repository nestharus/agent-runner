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
    let release_mode = mode.starts_with("held_release");
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
    let broker_generation = if mode == "broker_state"
        || mode == "held_prepared"
        || mode == "held_guardian_death"
        || release_mode
    {
        let sidecar_dir = broker_state.join("sidecar");
        fs::create_dir(&sidecar_dir).unwrap();
        fs::set_permissions(&sidecar_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let target = sidecar_dir.join("pid-identity.db");
        rusqlite::Connection::open(data.join("pid-identity.db"))
            .unwrap()
            .execute("VACUUM INTO ?1", [target.to_str().unwrap()])
            .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let proof = oulipoly_state::mailbox::QuiescedCutoverProof::private_fixture();
        Some(BrokerSidecar::activate_private_fixture_copy(&target, &broker_state, &proof).unwrap())
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
        .envs(
            (mode == "held_release_gate_fail")
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_GATE_WRITE_V1", "1")),
        )
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
    if mode == "held_prepared" || mode == "held_guardian_death" || release_mode {
        let generation = broker_generation.unwrap();
        // A copied v29 path is unusable before E and throughout W/R.
        fs::write(data.join("pid-identity.db"), b"retired copied owner").unwrap();
        let out = temp.path().join("held.out");
        let err = temp.path().join("held.err");
        let mut entry = Command::new(&runner)
            .arg("__age319-private-held-prepared-v30")
            .env("OULIPOLY_DATA_DIR", &data)
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .envs(
                (mode == "held_guardian_death")
                    .then_some(("AGE319_PRIVATE_EXPECT_GUARDIAN_DEATH_V1", "1")),
            )
            .envs(release_mode.then_some(("AGE319_PRIVATE_RELEASE_V30", "1")))
            .envs(
                (mode == "held_release_lost_reply")
                    .then_some(("AGE319_PRIVATE_RELEASE_LOST_REPLY_V1", "1")),
            )
            .env_remove("LD_LIBRARY_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).unwrap()))
            .stderr(Stdio::from(File::create(&err).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| gate.join("prepared").exists() || entry.try_wait().unwrap().is_some());
        assert!(
            gate.join("prepared").exists(),
            "entry: {} broker: {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        let prepared: oulipoly_state::mailbox::PreparedBrokerOwner =
            serde_json::from_slice(&fs::read(gate.join("prepared")).unwrap()).unwrap();
        assert_eq!(prepared.source_generation, generation);
        assert_eq!(prepared.entry.host_pid, entry.id() as i32);
        assert_eq!(prepared.domain_id, domain);
        assert_ne!(prepared.entry.pidns_ino, prepared.joined_child.pidns_ino);
        let entry_record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                broker_state
                    .join("entries")
                    .join(format!("{}.json", prepared.root_id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            entry_record["prepared_driver"]["host_pid"],
            prepared.driver.host_pid
        );
        assert_eq!(
            entry_record["prepared_driver"]["starttime_ticks"],
            prepared.driver.starttime_ticks
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        assert!(entry.try_wait().unwrap().is_none());
        let persisted = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            &broker_state,
        )
        .unwrap()
        .read_exact_prepared_owner(&generation, &prepared.root_id, &prepared.owner_generation)
        .unwrap();
        assert_eq!(persisted, prepared);
        assert!(
            BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .is_err()
        );
        let db = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let running: i64 = db
            .query_row(
                "SELECT count(*) FROM completion_continuation_owner",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(running, 0);
        let read = protocol::StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::read_prepared_owner_at(&socket, &read).is_err()); // wrong actor
        let child_attest = protocol::StateReadSpec {
            protocol: "broker-release-attest-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::attest_released_child_at(&socket, &child_attest).is_err());
        let forged = protocol::StateWriteSpec {
            protocol: "broker-prepared-write-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            action: protocol::StateWriteAction::Prepare {
                driver_pid: prepared.driver.host_pid,
                endpoint: prepared.endpoint.clone(),
            },
        };
        assert!(protocol::prepare_owner_at(&socket, &forged).is_err());
        let mut wrong = read;
        wrong.source_generation = uuid::Uuid::new_v4().to_string();
        assert!(protocol::read_prepared_owner_at(&socket, &wrong).is_err());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let wrong_release = protocol::StateWriteSpec {
            protocol: "broker-held-release-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            action: protocol::StateWriteAction::Release,
        };
        assert!(protocol::release_prepared_owner_at(&socket, &wrong_release).is_err());
        if mode == "held_guardian_death" {
            unsafe {
                libc::kill(prepared.guardian.host_pid, libc::SIGKILL);
            }
            fs::write(gate.join("guardian-dead"), b"yes").unwrap();
            eventually(|| {
                gate.join("death-refused").exists() || entry.try_wait().unwrap().is_some()
            });
            assert!(
                gate.join("death-refused").exists(),
                "{}",
                fs::read_to_string(&err).unwrap()
            );
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        }
        if release_mode {
            if mode == "held_release_commit_fail" {
                rusqlite::Connection::open(broker_state.join("sidecar/pid-identity.db"))
                    .unwrap()
                    .execute_batch("CREATE TRIGGER age319_release_fault BEFORE INSERT ON broker_owner_release BEGIN SELECT RAISE(ABORT, 'fixture commit fault'); END;")
                    .unwrap();
            }
            if mode == "held_release_child_predeath" {
                unsafe {
                    libc::kill(prepared.joined_child.host_pid, libc::SIGKILL);
                }
            }
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| gate.join("released").exists() || entry.try_wait().unwrap().is_some());
            if mode == "held_release_commit_fail"
                || mode == "held_release_child_predeath"
                || mode == "held_release_gate_fail"
            {
                assert!(!gate.join("released").exists());
                let errors = fs::read_to_string(&err).unwrap();
                if mode == "held_release_commit_fail" {
                    assert!(errors.contains("fixture commit fault"), "{errors}");
                }
                if mode == "held_release_gate_fail" {
                    assert!(
                        errors.contains("Broken pipe") || errors.contains("os error 32"),
                        "{errors}"
                    );
                }
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                assert!(
                    BrokerSidecar::open_existing(
                        &broker_state.join("sidecar/pid-identity.db"),
                        &broker_state
                    )
                    .unwrap()
                    .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
                    .is_err()
                );
                let still_zero: i64 = db
                    .query_row(
                        "SELECT count(*) FROM completion_continuation_owner",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(still_zero, 0);
                stop(&mut broker);
                unsafe {
                    libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
                }
                return;
            }
            assert!(
                gate.join("released").exists(),
                "entry: {} broker: {}",
                fs::read_to_string(&err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
            let evidence: oulipoly_state::mailbox::BrokerReleaseEvidence =
                serde_json::from_slice(&fs::read(gate.join("released")).unwrap()).unwrap();
            assert_eq!(evidence.prepared, prepared);
            assert_eq!(evidence.owner.owner_generation, prepared.owner_generation);
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            assert_eq!(
                db.query_row::<i64, _, _>(
                    "SELECT count(*) FROM completion_continuation_owner",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
                1
            );
            eventually(|| {
                gate.join("child-attested").exists() || entry.try_wait().unwrap().is_some()
            });
            assert!(
                gate.join("child-attested").exists(),
                "child: {} broker: {}",
                fs::read_to_string(&err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
            assert_eq!(
                fs::read_to_string(gate.join("child-attested")).unwrap(),
                evidence.release_id
            );
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            let durable = BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .unwrap();
            assert_eq!(durable, evidence);
            if mode == "held_release_guardian_death" {
                unsafe {
                    libc::kill(prepared.guardian.host_pid, libc::SIGKILL);
                }
            }
            if mode == "held_release_child_death" {
                unsafe {
                    libc::kill(prepared.joined_child.host_pid, libc::SIGKILL);
                }
            }
            if mode == "held_release_broker_death" {
                stop(&mut broker);
                let premature_restart_log = temp.path().join("released-child-restart.log");
                let mut premature_restart =
                    Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .stderr(Stdio::from(File::create(&premature_restart_log).unwrap()))
                        .spawn()
                        .unwrap();
                eventually(|| {
                    protocol::request_at(&socket, Operation::Classify).is_ok()
                        || premature_restart.try_wait().unwrap().is_some()
                });
                assert!(
                    premature_restart.try_wait().unwrap().is_none(),
                    "{}",
                    fs::read_to_string(&premature_restart_log).unwrap()
                );
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                std::thread::sleep(Duration::from_millis(200));
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                stop(&mut premature_restart);
            }
            if mode != "held_release_broker_death" {
                fs::write(gate.join("child-effect"), b"yes").unwrap();
            }
            if mode == "held_release" || mode == "held_release_lost_reply" {
                eventually(|| {
                    fs::metadata(&out).unwrap().len() > 0 || entry.try_wait().unwrap().is_some()
                });
                assert_eq!(
                    fs::read_to_string(&out).unwrap(),
                    format!("OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n", evidence.release_id)
                );
            } else {
                std::thread::sleep(Duration::from_millis(200));
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            }
            if mode != "held_release_broker_death" {
                stop(&mut broker);
            }
            fs::write(gate.join("finish"), b"done").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            let _ = entry.wait();
            let restart_log = temp.path().join("released-restart.log");
            let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                .spawn()
                .unwrap();
            eventually(|| {
                protocol::request_at(&socket, Operation::Classify).is_ok()
                    || restarted.try_wait().unwrap().is_some()
            });
            assert!(restarted.try_wait().unwrap().is_none());
            let second = Command::new(&runner)
                .arg("__age319-private-held-prepared-v30")
                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                .output()
                .unwrap();
            assert!(!second.status.success());
            assert_eq!(
                fs::read_dir(broker_state.join("entries")).unwrap().count(),
                1
            );
            stop(&mut restarted);
            unsafe {
                libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
            }
            return;
        }
        stop(&mut broker);
        fs::write(gate.join("finish"), b"done").unwrap();
        eventually(|| entry.try_wait().unwrap().is_some());
        assert!(!entry.wait().unwrap().success());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let records: Vec<_> = fs::read_dir(broker_state.join("entries"))
            .unwrap()
            .collect();
        assert_eq!(records.len(), 1);
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap())
                .unwrap();
        assert_eq!(record["join_consumed"], true);
        assert_eq!(
            record["joined_child"]["host_pid"],
            prepared.joined_child.host_pid
        );
        let restart_log = temp.path().join("prepared-restart.log");
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
        let retained = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            &broker_state,
        )
        .unwrap()
        .read_exact_prepared_owner(&generation, &prepared.root_id, &prepared.owner_generation)
        .unwrap();
        assert_eq!(retained, prepared);
        assert!(
            BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .is_err()
        );
        let second = Command::new(&runner)
            .arg("__age319-private-held-prepared-v30")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .output()
            .unwrap();
        assert!(!second.status.success());
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        stop(&mut restarted);
        unsafe {
            libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
        }
        return;
    }
    if let Some(_generation) = broker_generation {
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
            String::from_utf8_lossy(&first.stderr).contains("installed Runner has no v30 route"),
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
            String::from_utf8_lossy(&unavailable.stderr)
                .contains("installed broker entry gate unavailable")
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
        assert!(
            String::from_utf8_lossy(&second.stderr).contains("installed Runner has no v30 route")
        );
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
    if mode == "held_death" {
        // J has consumed its one use and fsynced the exact child, but the
        // broker still owns the pre-exec gate. Losing the broker must deliver
        // EOF, not an executable release or a second child.
        stop(&mut broker);
        eventually(|| entry.try_wait().unwrap().is_some());
        assert!(!entry.wait().unwrap().success());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap())
                .unwrap();
        assert_eq!(persisted["join_consumed"], true);
        assert_eq!(persisted["joined_child"]["host_pid"], child_pid);
        let restart_log = temp.path().join("held-restart.log");
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
            "held restart: {}",
            fs::read_to_string(&restart_log).unwrap()
        );
        assert!(
            protocol::request_at(&socket, Operation::ReserveEntry)
                .unwrap()
                .starts_with("error ")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        stop(&mut restarted);
        unsafe {
            libc::kill(init_pid, libc::SIGKILL);
        }
        return;
    }
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
    for mode in [
        "help",
        "diagnostics",
        "join_only",
        "held_death",
        "broker_state",
        "held_prepared",
        "held_guardian_death",
        "held_release",
        "held_release_broker_death",
        "held_release_guardian_death",
        "held_release_child_death",
        "held_release_lost_reply",
        "held_release_commit_fail",
        "held_release_child_predeath",
        "held_release_gate_fail",
    ] {
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
