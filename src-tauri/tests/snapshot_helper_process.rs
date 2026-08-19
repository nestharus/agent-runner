use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn runner_executes_physical_snapshot_protocol_before_cli_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.db");
    let destination = directory.path().join("destination.db");
    let control = directory.path().join("control");
    std::fs::create_dir(&control).unwrap();
    std::fs::write(&source, "physical snapshot bytes").unwrap();
    std::fs::write(control.join("compare"), []).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .arg("__oulipoly-snapshot-helper")
        .arg(&source)
        .arg(&destination)
        .arg(&control)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("snapshot helper did not finish");
        }
        std::thread::sleep(Duration::from_millis(2));
    };

    assert!(status.success(), "snapshot helper failed: {status}");
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"physical snapshot bytes"
    );
    assert_eq!(
        std::fs::read_to_string(control.join("result")).unwrap(),
        "stable\n"
    );
}
