use std::process::Command;

#[test]
fn runner_executes_physical_snapshot_protocol_before_cli_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.db");
    let destination = directory.path().join("destination.db");
    let control = directory.path().join("control");
    std::fs::create_dir(&control).unwrap();
    std::fs::write(&source, "physical snapshot bytes").unwrap();
    std::fs::write(control.join("compare"), []).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg("__oulipoly-snapshot-helper")
        .arg(&source)
        .arg(&destination)
        .arg(&control)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "snapshot helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"physical snapshot bytes"
    );
    assert_eq!(
        std::fs::read_to_string(control.join("result")).unwrap(),
        "stable\n"
    );
}
