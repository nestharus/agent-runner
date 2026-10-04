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
//! ingress, the owner is unreachable, or it refused (nothing ran); 71 when
//! the launch failed after acceptance; 75 when the run was accepted and its
//! end is not known (`end-unknown`, `left-to-successor`, or the connection
//! ended without a final line). A missing end is never 0. There is no
//! fallback to any other owner, Broker or local execution.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use oulipoly_root_supervisor::bash::{BASH_ENV, PROTOCOL, unbase64};
use serde_json::{Value, json};

const EX_UNAVAILABLE: u8 = 69;
const EX_OSERR: u8 = 71;
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
    let mut accepted = false;
    let mut stdout = std::io::stdout().lock();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            say(&json!({ "outcome": "protocol-violation", "line": line }));
            return ExitCode::from(if accepted {
                EX_TEMPFAIL
            } else {
                EX_UNAVAILABLE
            });
        };
        match event["event"].as_str().unwrap_or_default() {
            "refused" => {
                say(&event);
                return ExitCode::from(EX_UNAVAILABLE);
            }
            "accepted" => {
                accepted = true;
                say(&event);
            }
            "output" => {
                let bytes = event["b64"].as_str().and_then(unbase64).unwrap_or_default();
                let _ = stdout.write_all(&bytes).and_then(|()| stdout.flush());
            }
            "started" | "output-closed" => {}
            "end" => {
                say(&event);
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
                return ExitCode::from(code.unwrap_or(EX_TEMPFAIL));
            }
            "launch-failed" => {
                say(&event);
                return ExitCode::from(EX_OSERR);
            }
            _ => {
                // `end-unknown`, `left-to-successor`, or anything newer.
                say(&event);
                return ExitCode::from(EX_TEMPFAIL);
            }
        }
    }
    say(&json!({
        "outcome": "no-final-line",
        "meaning": if accepted { "accepted-end-unknown" } else { "not-accepted" },
        "request_sent": sent,
    }));
    ExitCode::from(if accepted {
        EX_TEMPFAIL
    } else {
        EX_UNAVAILABLE
    })
}
