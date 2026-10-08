//! Prototype requester for a root's Bash ingress: what a Bash entry inside
//! a harness of this lineage does to run one command under its own root's
//! owner. It stands in for the Bash tool here; the Bash tool itself does
//! not speak this protocol.
//!
//! `oulipoly-root-bash [--] ARGV...` runs ARGV in the current directory
//! through the ingress named by `OULIPOLY_ROOT_BASH_V1`, writes the run's
//! output (stderr joined) to stdout, and says on stderr, one JSON line each
//! prefixed `oulipoly-root-bash: `, how it was accepted and how it ended.
//! `--result rv1w:ROOT:WORK` instead reads that harness's original work
//! witness through the same positively attributed socket. JSON goes to
//! stdout; query exit 0 means a witness was read, not that the work succeeded.
//! One five-second deadline bounds socket connect, request write and reply
//! read, including partial replies. Timeout is unknown query knowledge, never
//! a command failure or permission to retry work. Query output failure uses
//! 74; usage/refusal/uncertainty use 64/69/75. Explicit root-store discard
//! expires the reference. A query never runs a command or authorizes retry.
//!
//! Exit: the run's own exit code; 128+N for signal N; 69 when there is no
//! ingress, connection failed before sending, or it refused (nothing ran);
//! 71 for a proven no-start after acceptance, including a positive ordered
//! `started.exec_error` once the physical wait and output are known;
//! 74 for failed/unproven output delivery despite a known wait;
//! 75 for uncertain launch/end, including a
//! connection lost before acceptance. Uncertainty never authorizes a retry.
//! A missing end or missing output closure is never 0. There is no
//! fallback to any other owner, Broker or local execution.
//! The stderr status channel preserves `started` and the actual `end` wait.
//! A positive exec error also reports `requested-program-exec-failed`: the
//! requested program did not begin executing, but accepted work remains in
//! custody and may have caused pre-exec setup effects; do not replay it.
//! Output failure (74) and uncertain end/protocol (75) retain priority over
//! that error's exit 71. Ordinary code:127 remains exit 127, with no inference
//! about start. Numeric process exits alone cannot distinguish these facts.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use oulipoly_root_supervisor::bash::{BASH_ENV, PROTOCOL, unbase64};
use serde_json::{Value, json};

const RESULT_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESULT_BYTES: usize = 1 << 20;

const EX_UNAVAILABLE: u8 = 69;
const EX_OSERR: u8 = 71;
const EX_IOERR: u8 = 74;
const EX_TEMPFAIL: u8 = 75;

fn say(value: &Value) {
    eprintln!("oulipoly-root-bash: {value}");
}

fn main() -> ExitCode {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().is_some_and(|arg| arg == "--result") {
        return ExitCode::from(read_result(&argv));
    }
    if argv.first().is_some_and(|arg| arg == "--") {
        argv.remove(0);
    }
    if argv.is_empty() {
        say(&json!({ "outcome": "usage", "detail": "no command" }));
        return ExitCode::from(64);
    }
    let Some(path) = std::env::var_os(BASH_ENV) else {
        say(&json!({ "outcome": "no-ingress", "detail": format!("{BASH_ENV} is not set") }));
        return ExitCode::from(EX_UNAVAILABLE);
    };
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd.display().to_string(),
        Err(error) => {
            say(&json!({ "outcome": "no-cwd", "detail": error.to_string() }));
            return ExitCode::from(EX_UNAVAILABLE);
        }
    };
    let mut stream = match UnixStream::connect(&path) {
        Ok(stream) => stream,
        Err(error) => {
            say(&json!({ "outcome": "owner-unreachable", "detail": error.to_string() }));
            return ExitCode::from(EX_UNAVAILABLE);
        }
    };
    let request = json!({ "v": PROTOCOL, "op": "run", "argv": argv, "cwd": cwd });
    // The owner may refuse (and close) before reading the request; its
    // refusal is still there to read, so a failed write is not the answer.
    let sent = writeln!(stream, "{request}")
        .and_then(|()| stream.flush())
        .is_ok();
    let mut stdout = std::io::stdout().lock();
    ExitCode::from(receive(BufReader::new(stream), &mut stdout, sent))
}

/// An actual in-root consumer of a finalized local work reference. The
/// reference is a locator; authorization is still live harness attribution
/// at the owner socket. A query never reexecutes the original command.
fn read_result(argv: &[String]) -> u8 {
    let Some(reference) = argv.get(1).filter(|_| argv.len() == 2) else {
        say(&json!({"outcome":"usage","detail":"--result rv1w:ROOT:WORK"}));
        return 64;
    };
    let parts: Vec<_> = reference.split(':').collect();
    let work = parts
        .get(2)
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n > 0);
    if parts.len() != 3 || parts[0] != "rv1w" || parts[1].is_empty() || work.is_none() {
        say(&json!({"outcome":"bad-reference"}));
        return 64;
    }
    let unknown = |stage: &str, error: Option<&io::Error>| {
        say(&json!({
            "outcome": "result-query-unavailable",
            "reference": reference,
            "stage": stage,
            "error_kind": error.map(|e| format!("{:?}", e.kind())),
            "knowledge": "unknown",
            "meaning": "no work outcome, output or publication inferred; query never authorizes work retry",
        }));
    };
    let Some(path) = std::env::var_os(BASH_ENV) else {
        unknown("no-ingress", None);
        return EX_UNAVAILABLE;
    };
    let deadline = Instant::now() + RESULT_QUERY_TIMEOUT;
    let mut socket = match ResultSocket::connect(Path::new(&path), deadline) {
        Ok(socket) => socket,
        Err(error) => {
            unknown("connect", Some(&error));
            return if error.kind() == io::ErrorKind::TimedOut {
                EX_TEMPFAIL
            } else {
                EX_UNAVAILABLE
            };
        }
    };
    let request = json!({"v":PROTOCOL,"op":"result","root_id":parts[1],"work":work});
    let reply = match socket.exchange(&request) {
        Ok(reply) => reply,
        Err(error) => {
            unknown("exchange", Some(&error));
            return EX_TEMPFAIL;
        }
    };
    if reply["event"] != "work-result" {
        say(&reply);
        return EX_UNAVAILABLE;
    }
    if writeln!(std::io::stdout().lock(), "{reply}")
        .and_then(|()| std::io::stdout().flush())
        .is_err()
    {
        unknown("local-output", None);
        return EX_IOERR;
    }
    0
}

/// Single-query socket lifetime. All readiness waits share one deadline;
/// partial progress and interruptions cannot renew it. Dropping it cancels
/// only this query connection, with no launch/recovery or signal operation.
struct ResultSocket {
    stream: UnixStream,
    deadline: Instant,
}

impl ResultSocket {
    fn connect(path: &Path, deadline: Instant) -> io::Result<Self> {
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: zero is a valid initial sockaddr_un representation.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if bytes.is_empty() || bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        // Nonblocking from creation: even a full listen backlog cannot trap
        // the caller inside connect. EAGAIN is unavailable, with no retry.
        // SAFETY: socket has integer arguments and returns a new owned fd.
        let fd = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the newly created descriptor has no other owner.
        let socket = Self {
            stream: unsafe { UnixStream::from_raw_fd(fd) },
            deadline,
        };
        let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
        // SAFETY: address is initialized and length covers its pathname + NUL.
        if unsafe { libc::connect(fd, (&raw const address).cast(), length as libc::socklen_t) } < 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(error);
            }
            socket.wait(libc::POLLOUT)?;
            if let Some(error) = socket.stream.take_error()? {
                return Err(error);
            }
        }
        Ok(socket)
    }

    fn wait(&self, events: libc::c_short) -> io::Result<()> {
        loop {
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or(io::ErrorKind::TimedOut)?;
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut poll = libc::pollfd {
                fd: self.stream.as_raw_fd(),
                events,
                revents: 0,
            };
            // SAFETY: poll receives one valid descriptor and a bounded wait.
            let ready = unsafe { libc::poll(&mut poll, 1, millis) };
            if ready > 0 {
                return Ok(());
            }
            if ready == 0 {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn exchange(&mut self, request: &Value) -> io::Result<Value> {
        let request = format!("{request}\n");
        let mut pending = request.as_bytes();
        while !pending.is_empty() {
            self.wait(libc::POLLOUT)?;
            match self.stream.write(pending) {
                Ok(0) => break,
                Ok(n) => pending = &pending[n..],
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                // The owner can refuse and close its read side before the
                // request write. Preserve a positive refusal already waiting;
                // failed send alone supplies no result knowledge.
                Err(_) => break,
            }
        }
        let mut reply = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            self.wait(libc::POLLIN)?;
            match self.stream.read(&mut chunk) {
                Ok(0) if reply.is_empty() => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(0) => {
                    return serde_json::from_slice(&reply)
                        .map_err(|_| io::ErrorKind::InvalidData.into());
                }
                Ok(n) => {
                    let end = chunk[..n].iter().position(|byte| *byte == b'\n');
                    reply.extend_from_slice(&chunk[..end.unwrap_or(n)]);
                    if reply.len() >= MAX_RESULT_BYTES {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    if end.is_some() {
                        return serde_json::from_slice(&reply)
                            .map_err(|_| io::ErrorKind::InvalidData.into());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }
}

fn receive(reader: impl BufRead, stdout: &mut impl Write, sent: bool) -> u8 {
    let mut accepted = false;
    let mut accepted_work = None;
    let mut started = false;
    let mut exec_failed = false;
    let mut protocol_failed = false;
    let mut output_seen = false;
    let mut bytes = 0u64;
    let mut closed = false;
    let mut output_failed = false;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            say(&json!({ "outcome": "protocol-violation", "meaning": "possible-effect-unknown" }));
            return EX_TEMPFAIL;
        };
        match event["event"].as_str().unwrap_or_default() {
            "refused" => {
                say(&event);
                return if accepted {
                    EX_TEMPFAIL
                } else {
                    EX_UNAVAILABLE
                };
            }
            "accepted" => {
                if accepted || started || output_seen {
                    protocol_failed = true;
                    say(
                        &json!({ "outcome": "protocol-violation", "reason": "accepted-out-of-order", "meaning": "possible-effect-unknown" }),
                    );
                } else if event["durable"] == true {
                    accepted_work = event["work"].as_u64().filter(|work| *work > 0);
                }
                accepted = true;
                say(&event);
            }
            "output" => {
                output_seen = true;
                let decoded = event["b64"].as_str().and_then(unbase64);
                let fault = match decoded {
                    None => Some("invalid-or-missing-base64".to_owned()),
                    Some(_) if closed => Some("output-after-closure".to_owned()),
                    Some(chunk) => {
                        bytes += chunk.len() as u64;
                        stdout
                            .write_all(&chunk)
                            .and_then(|()| stdout.flush())
                            .err()
                            .map(|error| error.to_string())
                    }
                };
                if let Some(reason) = fault {
                    output_failed = true;
                    say(&json!({ "outcome": "output-failed", "reason": reason }));
                }
            }
            "started" => {
                // This is a stage fact, not an inference from the wait or a
                // redundant top-level copy. Keep reading for the actual end
                // and output, even after a positive error or bad ordering.
                say(&event);
                let ordered =
                    accepted_work.is_some() && !started && !output_seen && !protocol_failed;
                started = true;
                let error = match event.get("exec_error") {
                    None | Some(Value::Null) => Some(None),
                    Some(Value::String(error)) if !error.trim().is_empty() => Some(Some(error)),
                    _ => None,
                };
                if !ordered || error.is_none() {
                    protocol_failed = true;
                    say(
                        &json!({ "outcome": "protocol-violation", "reason": "invalid-started-stage", "meaning": "possible-effect-unknown" }),
                    );
                } else if let Some(Some(error)) = error {
                    exec_failed = true;
                    say(&json!({
                        "outcome": "requested-program-exec-failed",
                        "exec_error": error,
                        "work": accepted_work,
                        "meaning": "the requested program did not begin executing; accepted work may have caused setup effects; do not replay",
                    }));
                }
            }
            "output-closed" => {
                output_seen = true;
                if closed || event["bytes"].as_u64() != Some(bytes) {
                    output_failed = true;
                    say(
                        &json!({ "outcome": "output-failed", "reason": "output-byte-count-mismatch" }),
                    );
                }
                closed = true;
            }
            "output-failed" => {
                output_seen = true;
                output_failed = true;
                say(&event);
            }
            "end" => {
                // Preserve the actual waiter evidence even when delivery fails.
                say(&event);
                if output_failed
                    || !closed
                    || event["output"]["state"] == "failed"
                    || event["output"]["state"] == "unknown"
                    || stdout.flush().is_err()
                {
                    say(
                        &json!({ "outcome": "output-failed", "reason": "output-delivery-not-proven", "wait_status": event["status"] }),
                    );
                    return EX_IOERR;
                }
                let status = event["status"].as_str().unwrap_or_default();
                let code = if let Some(code) = status.strip_prefix("code:") {
                    code.parse().ok()
                } else if let Some(signal) = status.strip_prefix("signal:") {
                    signal
                        .parse::<u8>()
                        .ok()
                        .map(|signal| 128u8.saturating_add(signal))
                } else {
                    None
                };
                return match code {
                    _ if protocol_failed => EX_TEMPFAIL,
                    None => EX_TEMPFAIL,
                    Some(_) if exec_failed => EX_OSERR,
                    Some(code) => code,
                };
            }
            "launch-failed" => {
                say(&event);
                return if event["not_started"] == true && !started && !protocol_failed {
                    EX_OSERR
                } else {
                    EX_TEMPFAIL
                };
            }
            _ => {
                // `end-unknown`, `left-to-successor`, or anything newer.
                say(&event);
                return EX_TEMPFAIL;
            }
        }
    }
    say(&json!({
        "outcome": "no-final-line",
        "meaning": if accepted { "accepted-end-unknown" } else { "possible-effect-unknown" },
        "request_sent": sent,
    }));
    EX_TEMPFAIL
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn result_query_preserves_a_positive_refusal_after_request_write_failure() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        peer.shutdown(std::net::Shutdown::Read).unwrap();
        peer.write_all(b"{\"event\":\"refused\",\"reason\":\"peer-unattributed\"}\n")
            .unwrap();
        let mut socket = ResultSocket {
            stream: client,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        assert_eq!(
            socket.stream.write(b"request").unwrap_err().kind(),
            io::ErrorKind::BrokenPipe,
            "actual send failure"
        );
        let reply = socket.exchange(&json!({"op":"result"})).unwrap();
        assert_eq!(reply["event"], "refused");
        assert_eq!(reply["reason"], "peer-unattributed");
    }

    #[test]
    fn result_query_deadline_cancels_a_peer_that_withholds_the_line_end() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        let mut socket = ResultSocket {
            stream: client,
            deadline: Instant::now() + Duration::from_millis(120),
        };
        let server = std::thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(&mut peer).read_line(&mut request).unwrap();
            let request: Value = serde_json::from_str(&request).unwrap();
            assert_eq!(request["op"], "result");
            peer.write_all(b"{\"event\":\"work-result\"").unwrap();
            // Keep the peer open and genuinely wait for client cancellation.
            let mut byte = [0];
            assert_eq!(peer.read(&mut byte).unwrap(), 0);
        });
        let start = Instant::now();
        let error = socket
            .exchange(&json!({"v":PROTOCOL,"op":"result","root_id":"original","work":7}))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(socket);
        server.join().unwrap();
    }

    #[test]
    fn result_query_deadline_also_bounds_a_peer_that_never_reads_the_request() {
        let (client, peer) = UnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        let buffer: libc::c_int = 4096;
        // SAFETY: setsockopt reads one initialized integer for this owned fd.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    client.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&raw const buffer).cast(),
                    std::mem::size_of_val(&buffer) as libc::socklen_t,
                )
            },
            0
        );
        let mut socket = ResultSocket {
            stream: client,
            deadline: Instant::now() + Duration::from_millis(120),
        };
        let error = socket
            .exchange(&json!({"op":"result","root_id":"x".repeat(1 << 20),"work":7}))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        drop(socket);
        drop(peer);
    }

    #[test]
    fn result_query_partial_progress_does_not_renew_the_deadline() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        client.set_nonblocking(true).unwrap();
        let mut socket = ResultSocket {
            stream: client,
            deadline: Instant::now() + Duration::from_millis(150),
        };
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(&mut peer).read_line(&mut request).unwrap();
            let mut chunks = 0;
            // An authored slow reply: continuing socket progress without a
            // newline. This tests the total bound, not a missing-work claim.
            loop {
                if peer.write_all(b" ").is_err() {
                    break;
                }
                chunks += 1;
                if stopped.recv_timeout(Duration::from_millis(20)).is_ok() {
                    break;
                }
            }
            chunks
        });
        let start = Instant::now();
        assert_eq!(
            socket.exchange(&json!({"op":"result"})).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(socket);
        let _ = stop.send(());
        assert!(
            server.join().unwrap() >= 2,
            "actual partial progress occurred"
        );
    }

    struct FaultWriter {
        flush: bool,
    }
    impl Write for FaultWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.flush {
                Ok(bytes.len())
            } else {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }

    fn transcript(events: &[Value]) -> Cursor<Vec<u8>> {
        Cursor::new(
            events
                .iter()
                .map(|event| format!("{event}\n"))
                .collect::<String>()
                .into_bytes(),
        )
    }

    #[test]
    fn output_faults_cannot_be_hidden_by_a_successful_wait() {
        let accepted = json!({ "event": "accepted" });
        let output = json!({ "event": "output", "b64": "aGkK" });
        let closed = json!({ "event": "output-closed", "bytes": 3 });
        let end = json!({ "event": "end", "status": "code:0", "output": { "state": "closed" } });
        let good = [
            accepted.clone(),
            output.clone(),
            closed.clone(),
            end.clone(),
        ];
        let mut bytes = Vec::new();
        assert_eq!(receive(transcript(&good), &mut bytes, true), 0);
        assert_eq!(bytes, b"hi\n");
        for fault in [
            json!({ "event": "output" }),
            json!({ "event": "output", "b64": "!!!!" }),
            json!({ "event": "output", "b64": "aG==aGkK" }),
            json!({ "event": "output-failed" }),
        ] {
            assert_eq!(
                receive(
                    transcript(&[accepted.clone(), fault, closed.clone(), end.clone()]),
                    &mut Vec::new(),
                    true
                ),
                74
            );
        }
        assert_eq!(
            receive(
                transcript(&[accepted.clone(), output.clone(), end.clone()]),
                &mut Vec::new(),
                true
            ),
            74
        );
        assert_eq!(
            receive(transcript(&[accepted, closed, end]), &mut Vec::new(), true),
            74,
            "missing bytes"
        );
        for flush in [false, true] {
            assert_eq!(
                receive(transcript(&good), &mut FaultWriter { flush }, true),
                74
            );
        }
    }

    #[test]
    fn missing_reply_or_uncertain_launch_never_means_not_run() {
        for sent in [false, true] {
            assert_eq!(receive(Cursor::new(b""), &mut Vec::new(), sent), 75);
        }
        for event in [
            json!({ "event": "launch-unknown" }),
            json!({ "event": "launch-failed" }),
        ] {
            assert_eq!(receive(transcript(&[event]), &mut Vec::new(), true), 75);
        }
        assert_eq!(
            receive(
                transcript(&[json!({ "event": "launch-failed", "not_started": true })]),
                &mut Vec::new(),
                true
            ),
            71
        );
        assert_eq!(
            receive(
                transcript(&[json!({ "event": "refused" })]),
                &mut Vec::new(),
                true
            ),
            69
        );
    }
}
