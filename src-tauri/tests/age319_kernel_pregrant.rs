//! The opt-in entry is an early hard gate, including for internal helper argv.
//! A failed preflight must not create State/mailbox storage or run maintenance.
#[cfg(target_os = "linux")]
#[test]
fn opt_in_pregrant_failure_leaves_live_storage_absent() {
    let data = tempfile::tempdir().unwrap();
    for argument in [
        "--help",
        "__completion-driver-v1",
        "__maintenance-worker-v1",
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
            .arg(argument)
            .env("OULIPOLY_DATA_DIR", data.path())
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{argument} unexpectedly passed the gate"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("OULIPOLY_KERNEL_ENTRY_GAP="),
            "{argument}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 0);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn broker_unavailable_after_read_only_preflight_does_not_open_state_or_migrate_mailbox() {
    let data = tempfile::tempdir().unwrap();
    let path = data.path().join("pid-identity.db");
    let mailbox =
        oulipoly_state::mailbox::MailboxDb::open_completion_continuation_domain(&path).unwrap();
    assert!(mailbox.completion_continuation_domain().unwrap().is_some());
    drop(mailbox);
    drop(oulipoly_state::StateDb::open(&data.path().join("state.db")).unwrap());
    let before = data_files(data.path());
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", data.path())
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("OULIPOLY_KERNEL_ENTRY_GAP="));
    assert_eq!(data_files(data.path()), before);
    assert!(data.path().join("state.db").exists());
}

#[cfg(target_os = "linux")]
fn data_files(path: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().to_string_lossy().into_owned();
            (name, std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[cfg(target_os = "linux")]
#[test]
fn ordinary_help_entry_remains_available() {
    let data = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", data.path())
        .env_remove("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn opt_in_direct_snapshot_helper_cannot_bypass_pre_main_gate() {
    let data = tempfile::tempdir().unwrap();
    let source = data.path().join("source.db");
    let destination = data.path().join("copied.db");
    let control = data.path().join("control");
    std::fs::write(&source, b"source bytes").unwrap();
    std::fs::create_dir(&control).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg("__oulipoly-snapshot-helper")
        .arg(&source)
        .arg(&destination)
        .arg(&control)
        .env("OULIPOLY_DATA_DIR", data.path())
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!destination.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"source bytes");
}
