//! Prototype requester for a root's Bash ingress: what a Bash entry inside
//! a harness of this lineage does to run one command under its own root's
//! owner. It stands in for the Bash tool here; the Bash tool itself does
//! not speak this protocol.
//!
//! `oulipoly-root-bash [--] ARGV...` runs ARGV in the current directory
//! through the ingress named by `OULIPOLY_ROOT_BASH_V1`, writes the run's
//! output (stderr joined) to stdout, and says on stderr, one JSON line each
//! prefixed `oulipoly-root-bash: `, how it was accepted and how it ended.
//!
//! Exit: the run's own exit code; 128+N for signal N; 69 when there is no
//! ingress, connection failed before sending, or it refused (nothing ran);
//! 71 for a proven no-start after acceptance; 74 for failed/unproven output
//! delivery despite a known wait; 75 for uncertain launch/end, including a
//! connection lost before acceptance. Uncertainty never authorizes a retry.
//! A missing end or missing output closure is never 0. There is no
//! fallback to any other owner, Broker or local execution.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use oulipoly_root_supervisor::bash::{BASH_ENV, PROTOCOL, unbase64};
use serde_json::{Value, json};

const EX_UNAVAILABLE: u8 = 69;
const EX_OSERR: u8 = 71;
const EX_IOERR: u8 = 74;
const EX_TEMPFAIL: u8 = 75;

fn say(value: &Value) {
    eprintln!("oulipoly-root-bash: {value}");
}

fn main() -> ExitCode {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
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

fn receive(reader: impl BufRead, stdout: &mut impl Write, sent: bool) -> u8 {
    let mut accepted = false;
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
                accepted = true;
                say(&event);
            }
            "output" => {
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
            "started" => {}
            "output-closed" => {
                if closed || event["bytes"].as_u64() != Some(bytes) {
                    output_failed = true;
                    say(
                        &json!({ "outcome": "output-failed", "reason": "output-byte-count-mismatch" }),
                    );
                }
                closed = true;
            }
            "output-failed" => {
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
                return code.unwrap_or(EX_TEMPFAIL);
            }
            "launch-failed" => {
                say(&event);
                return if event["not_started"] == true {
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
