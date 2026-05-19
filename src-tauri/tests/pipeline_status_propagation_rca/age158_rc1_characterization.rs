use std::process::{Command, Stdio};
use std::time::Duration;

use super::wait_with_timeout;

#[test]
fn age158_wait_with_timeout_assembles_status_stdout_and_stderr() {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg("printf 'known stdout\\n'; printf 'known stderr\\n' >&2; exit 7")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = wait_with_timeout(cmd.spawn().unwrap(), Duration::from_secs(2))
        .expect("child should exit before timeout");

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"known stdout\n");
    assert_eq!(output.stderr, b"known stderr\n");
}

#[test]
fn age158_wait_with_timeout_returns_none_after_timeout_even_with_pipe_output() {
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg("printf 'stdout before sleep\\n'; printf 'stderr before sleep\\n' >&2; sleep 5")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = wait_with_timeout(cmd.spawn().unwrap(), Duration::from_millis(100));

    assert!(output.is_none());
}
