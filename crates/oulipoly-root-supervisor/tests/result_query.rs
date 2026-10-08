//! Ordinary local query processes against owned synthetic socket peers.
//! These peers exercise cancellation/knowledge, not owner authorization;
//! the in-root process controls separately exercise the real reader fences.
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CLIENT: &str = env!("CARGO_BIN_EXE_oulipoly-root-bash");

#[test]
fn stalled_result_peer_is_cancelled_without_a_command_or_invented_result() {
    let dir = std::env::temp_dir().join(format!("u220-query-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("result.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(15)))
            .unwrap();
        let mut request = String::new();
        BufReader::new(&mut peer).read_line(&mut request).unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(
            request,
            serde_json::json!({"v":1,"op":"result","root_id":"original","work":7})
        );
        // Deliberately incomplete JSON; this live peer never completes a reply.
        peer.write_all(b"{\"event\":\"work-result\"").unwrap();
        let mut next = [0];
        assert_eq!(
            peer.read(&mut next).unwrap(),
            0,
            "client closed only its query socket"
        );
        request
    });
    let start = Instant::now();
    let result = Command::new(CLIENT)
        .args(["--result", "rv1w:original:7"])
        .env("OULIPOLY_ROOT_BASH_V1", &path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    let request = server.join().unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
    eprintln!(
        "stalled-query: elapsed={elapsed:?} exit={:?} stdout={:?} stderr={} request={request}",
        result.status.code(),
        result.stdout,
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.status.code(), Some(75));
    assert!(
        elapsed < Duration::from_secs(12),
        "one five-second query deadline"
    );
    assert!(result.stdout.is_empty(), "no invented witness or output");
    let diagnostic: Value = serde_json::from_str(
        String::from_utf8_lossy(&result.stderr)
            .trim()
            .strip_prefix("oulipoly-root-bash: ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(diagnostic["knowledge"], "unknown");
    assert_eq!(diagnostic["reference"], "rv1w:original:7");
    assert_eq!(diagnostic["error_kind"], "TimedOut");
}

#[test]
fn unavailable_result_peer_reports_unknown_query_knowledge() {
    let path = std::env::temp_dir().join(format!("u220-absent-{}.sock", std::process::id()));
    assert!(!path.exists());
    let start = Instant::now();
    let result = Command::new(CLIENT)
        .args(["--result", "rv1w:original:7"])
        .env("OULIPOLY_ROOT_BASH_V1", &path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    eprintln!(
        "unavailable-query: exit={:?} stdout={:?} stderr={}",
        result.status.code(),
        result.stdout,
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.status.code(), Some(69));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(result.stdout.is_empty());
    let diagnostic: Value = serde_json::from_str(
        String::from_utf8_lossy(&result.stderr)
            .trim()
            .strip_prefix("oulipoly-root-bash: ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(diagnostic["knowledge"], "unknown");
    assert_eq!(diagnostic["reference"], "rv1w:original:7");
    assert_eq!(diagnostic["stage"], "connect");
}

#[test]
fn saturated_result_listen_backlog_is_unavailable_without_blocking_connect() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    let dir = std::env::temp_dir().join(format!("u220-backlog-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("result.sock");
    let listener = UnixListener::bind(&path).unwrap();
    // SAFETY: listen changes only this test's owned socket queue to zero.
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let queued = UnixStream::connect(&path).unwrap();
    let start = Instant::now();
    let result = Command::new(CLIENT)
        .args(["--result", "rv1w:original:7"])
        .env("OULIPOLY_ROOT_BASH_V1", &path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    drop(queued);
    drop(listener);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&dir).unwrap();
    eprintln!(
        "backlog-query: exit={:?} stderr={}",
        result.status.code(),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.status.code(), Some(69));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("WouldBlock"));
}
