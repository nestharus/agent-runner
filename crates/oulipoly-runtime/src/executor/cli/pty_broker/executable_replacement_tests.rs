//! Controlled live-process replacement: fixture sockets and mailbox only.
use super::*;
use oulipoly_state::paths;

const FIXTURE: &str = "OULIPOLY_TEST_AGE344_FIXTURE";
const CHILD_TEST: &str =
    "executor::cli::pty_broker::executable_replacement_tests::replacement_child";

#[test]
fn live_replacement_preserves_adjacent_roots_and_submission() {
    run_replacement_fixture(true);
}

#[test]
fn live_replacement_preserves_environment_roots_and_submission() {
    run_replacement_fixture(false);
}

fn write_paths(directory: &Path, data: &Path, config: &Path) {
    fs::write(
        directory.join(paths::ADJACENT_PATHS_FILE_NAME),
        format!("data_dir = {:?}\nconfig_home = {:?}\n", data, config),
    )
    .unwrap();
}

fn run_replacement_fixture(adjacent: bool) {
    // A hard link provides a disposable launch name without copying binary bytes.
    // Keep it on the build filesystem; never replace the actual test executable.
    let executable = std::env::current_exe().unwrap();
    let fixture = tempfile::tempdir_in(executable.parent().unwrap()).unwrap();
    let root = fixture.path();
    let launch = root.join("runner");
    fs::hard_link(&executable, &launch).unwrap();
    let data = root.join(if adjacent {
        "adjacent-data"
    } else {
        "env-data"
    });
    let config = root.join(if adjacent {
        "adjacent-config"
    } else {
        "env-config"
    });
    fs::create_dir_all(&data).unwrap();
    if adjacent {
        write_paths(root, &data, &config);
    }
    let status = Command::new(&launch)
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(FIXTURE, root)
        .env(
            "OULIPOLY_TEST_AGE344_ADJACENT",
            if adjacent { "1" } else { "0" },
        )
        .env(paths::DATA_DIR_ENV, root.join("env-data"))
        .env(paths::CONFIG_HOME_ENV, root.join("env-config"))
        .status()
        .unwrap();
    assert!(status.success(), "replacement fixture failed: {status}");
}

#[test]
fn replacement_child() {
    let Some(root) = std::env::var_os(FIXTURE).map(PathBuf::from) else {
        return;
    };
    let adjacent = std::env::var("OULIPOLY_TEST_AGE344_ADJACENT").unwrap() == "1";
    let data = root.join(if adjacent {
        "adjacent-data"
    } else {
        "env-data"
    });
    let config = root.join(if adjacent {
        "adjacent-config"
    } else {
        "env-config"
    });
    // Live brokers have already resolved their runtime roots before accepting traffic.
    assert_eq!(paths::data_dir().unwrap(), data);
    assert_eq!(
        paths::config_dir().unwrap(),
        config.join(paths::APP_DATA_DIR_NAME)
    );
    let attempt = "age344-live-attempt";
    let mailbox_path = seed_test_mailbox_delivery(&data, attempt);

    let launch = root.join("runner");
    assert_eq!(std::env::current_exe().unwrap(), launch);
    fs::remove_file(&launch).unwrap();
    let decoy = root.join("unrelated-installation");
    fs::create_dir(&decoy).unwrap();
    fs::write(decoy.join("runner"), "not the running executable").unwrap();
    write_paths(
        &decoy,
        &decoy.join("wrong-data"),
        &decoy.join("wrong-config"),
    );
    std::os::unix::fs::symlink(decoy.join("runner"), &launch).unwrap();
    assert!(
        std::env::current_exe()
            .unwrap()
            .as_os_str()
            .as_bytes()
            .ends_with(b" (deleted)")
    );
    assert_eq!(paths::data_dir().unwrap(), data);
    assert_eq!(
        paths::config_dir().unwrap(),
        config.join(paths::APP_DATA_DIR_NAME)
    );

    let envelope = format!("notify\n[OULIPOLY-DELIVERY {attempt}]");
    // Same recipient but wrong invocation must still fail before any submission.
    assert_eq!(
        prepare_control_payload(
            envelope.as_bytes().to_vec(),
            Some(("session-a", "wrong-invocation"))
        )
        .err()
        .unwrap(),
        "mailbox_delivery_target_mismatch"
    );
    assert_eq!(
        prepare_control_payload(
            envelope.as_bytes().to_vec(),
            Some(("wrong-session", "invocation-a"))
        )
        .err()
        .unwrap(),
        "mailbox_delivery_target_mismatch"
    );
    let db = MailboxDb::open_default_if_exists().unwrap().unwrap();
    assert_eq!(db.path(), mailbox_path);
    assert!(!db.delivery_attempt_submission_started(attempt).unwrap());
    drop(db);

    let (mut client, mut server) = UnixStream::pair().unwrap();
    write_inject_frame(&mut client, envelope.as_bytes()).unwrap();
    let (master, mut receiver) = UnixStream::pair().unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut line = InputLineState::default();
    let mut output = ChildOutputState::default();
    let mut pending = PendingChildInput::new();
    let mut io = ControlRequestIo {
        master_fd: master.as_raw_fd(),
        child_pid: None,
        line_state: &mut line,
        child_output_state: &mut output,
        pending_child_input: &mut pending,
    };
    assert_eq!(
        process_control_request_with_pending(
            &mut server,
            &mut io,
            Some(("session-a", "invocation-a"))
        ),
        Ok(ControlPayloadOutcome::Accepted(Some(attempt.into())))
    );
    drop(master);
    let mut received = String::new();
    receiver.read_to_string(&mut received).unwrap();
    assert!(
        received.contains(&format!("[OULIPOLY-DELIVERY {attempt}]")),
        "{received}"
    );
    assert!(received.ends_with('\r'), "{received:?}");
    let db = MailboxDb::open_default_if_exists().unwrap().unwrap();
    let window = db.delivery_attempt_window(attempt).unwrap().unwrap();
    assert!(window.submission_started_at.is_some());
    assert!(window.acknowledged_at.is_some());
    // This API resolves the transport attempt, not native recipient consumption.
    assert!(window.resolved_at.is_some());
    assert!(!decoy.join("wrong-data").exists());
    if adjacent {
        fs::write(root.join(paths::ADJACENT_PATHS_FILE_NAME), "malformed = [").unwrap();
        assert!(
            paths::data_dir()
                .unwrap_err()
                .contains("Could not parse runtime paths file")
        );
    }
    println!(
        "AGE344: deleted executable, original roots, wrong-target rejection, socket submission and transport ACK verified; no consumption claim"
    );
}
