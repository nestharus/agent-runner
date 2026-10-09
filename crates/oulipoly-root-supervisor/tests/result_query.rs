//! Ordinary local query processes against owned synthetic socket peers.
//! These peers exercise cancellation/knowledge, not owner authorization;
//! the in-root process controls separately exercise the real reader fences.
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CLIENT: &str = env!("CARGO_BIN_EXE_oulipoly-root-bash");

#[derive(Clone, Copy, Debug)]
enum DiagnosticSink {
    Capture,
    Full,
    Disconnected,
}

impl DiagnosticSink {
    fn stdio(self) -> Stdio {
        match self {
            Self::Capture => Stdio::piped(),
            Self::Full => Stdio::from(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            ),
            Self::Disconnected => {
                let (writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
                drop(reader);
                Stdio::from(std::os::fd::OwnedFd::from(writer))
            }
        }
    }
}

/// Only the requester and an owned synthetic socket peer run. No owner,
/// namespace wrapper, provider, command launch or production store is used.
fn diagnostic_control(
    args: &[&str],
    reply: Option<&[u8]>,
    stdout_full: bool,
    sink: DiagnosticSink,
) -> std::process::Output {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "u289-query-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    assert!(!path.exists());
    let server = reply.map(|reply| {
        let listener = UnixListener::bind(&path).unwrap();
        let reply = reply.to_vec();
        std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(12)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&mut peer).read_line(&mut request).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap(),
                serde_json::json!({"v":1,"op":"result","root_id":"original","work":7})
            );
            peer.write_all(&reply).unwrap();
            // Leave the partial reply open to exercise the existing socket
            // deadline; otherwise EOF is a deliberate exchange condition.
            if reply != b"{\"event\":\"work-result\"" {
                peer.shutdown(std::net::Shutdown::Write).unwrap();
            }
            let mut byte = [0];
            assert_eq!(
                peer.read(&mut byte).unwrap(),
                0,
                "query connection released"
            );
            request
        })
    });
    let mut command = Command::new(CLIENT);
    command
        .args(args)
        .env_remove("OULIPOLY_ROOT_BASH_V1")
        .stdin(Stdio::null())
        .stderr(sink.stdio());
    // A missing path is distinct from an unset ingress environment.
    if args != ["--result", "rv1w:no-ingress:7"] {
        command.env("OULIPOLY_ROOT_BASH_V1", &path);
    }
    if stdout_full {
        command.stdout(DiagnosticSink::Full.stdio());
    }
    let output = command.output().unwrap();
    if let Some(server) = server {
        let request = server.join().unwrap();
        std::fs::remove_file(&path).unwrap();
        eprintln!("query request={request:?}; connection released; socket removed");
    }
    assert!(!path.exists());
    eprintln!(
        "query args={args:?} sink={sink:?} stdout_full={stdout_full} exit={:?} stdout={:?} stderr={:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn query_outcomes_survive_diagnostic_write_failure() {
    let witness = b"{\"event\":\"work-result\",\"status\":\"code:7\"}\n";
    let refusal = b"{\"event\":\"refused\",\"reason\":\"peer-unattributed\"}\n";
    let mut mismatches = Vec::new();
    for sink in [
        DiagnosticSink::Capture,
        DiagnosticSink::Full,
        DiagnosticSink::Disconnected,
    ] {
        for (args, reply, stdout_full, code, diagnostic) in [
            (&["--result"][..], None, false, 64, "usage"),
            (
                &["--result", "invalid"][..],
                None,
                false,
                64,
                "bad-reference",
            ),
            (
                &["--result", "rv1w:no-ingress:7"][..],
                None,
                false,
                69,
                "no-ingress",
            ),
            (
                &["--result", "rv1w:original:7"][..],
                None,
                false,
                69,
                "connect",
            ),
            (
                &["--result", "rv1w:original:7"][..],
                Some(&refusal[..]),
                false,
                69,
                "peer-unattributed",
            ),
            (
                &["--result", "rv1w:original:7"][..],
                Some(&b""[..]),
                false,
                75,
                "UnexpectedEof",
            ),
            (
                &["--result", "rv1w:original:7"][..],
                Some(&b"invalid\n"[..]),
                false,
                75,
                "InvalidData",
            ),
            (
                &["--result", "rv1w:original:7"][..],
                Some(&witness[..]),
                true,
                74,
                "local-output",
            ),
            (
                &["--result", "rv1w:original:7"][..],
                Some(&witness[..]),
                false,
                0,
                "",
            ),
        ] {
            let output = diagnostic_control(args, reply, stdout_full, sink);
            if output.status.code() != Some(code) {
                mismatches.push(format!(
                    "{args:?} {sink:?} stdout_full={stdout_full}: expected {code}, got {:?}",
                    output.status
                ));
            }
            assert_eq!(
                output.stdout,
                if code == 0 { &witness[..] } else { &[][..] }
            );
            if matches!(sink, DiagnosticSink::Capture) {
                let text = String::from_utf8_lossy(&output.stderr);
                if code == 0 {
                    assert!(text.is_empty());
                } else {
                    assert!(text.contains(diagnostic), "{text}");
                }
                if matches!(code, 74 | 75) || diagnostic == "connect" || diagnostic == "no-ingress"
                {
                    assert!(text.contains("\"knowledge\":\"unknown\""), "{text}");
                }
            } else {
                assert!(
                    output.stderr.is_empty(),
                    "failed sink cannot deliver diagnostics"
                );
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn query_timeout_survives_diagnostic_write_failure() {
    let mut mismatches = Vec::new();
    for sink in [
        DiagnosticSink::Capture,
        DiagnosticSink::Full,
        DiagnosticSink::Disconnected,
    ] {
        let start = Instant::now();
        let output = diagnostic_control(
            &["--result", "rv1w:original:7"],
            Some(b"{\"event\":\"work-result\""),
            false,
            sink,
        );
        assert!(start.elapsed() < Duration::from_secs(12));
        assert!(output.stdout.is_empty());
        if output.status.code() != Some(75) {
            mismatches.push(format!("{sink:?}: {:?}", output.status));
        }
        if matches!(sink, DiagnosticSink::Capture) {
            let text = String::from_utf8_lossy(&output.stderr);
            assert!(
                text.contains("TimedOut") && text.contains("\"knowledge\":\"unknown\""),
                "{text}"
            );
        } else {
            assert!(output.stderr.is_empty());
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

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
