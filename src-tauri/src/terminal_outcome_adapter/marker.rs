//! Terminal-signal marker formatting and stderr emission.
//!
//! ## Declared roles
//!
//! `formatter`

use oulipoly_runtime::executor::TerminalSignal;
use std::io;
use uuid::Uuid;

pub fn emit_terminal_signal_marker(
    signal: &TerminalSignal,
    invocation_id: &Uuid,
    session_id: Option<&Uuid>,
    stderr: &mut impl io::Write,
) -> io::Result<()> {
    let payload = terminal_signal_marker_payload(signal, invocation_id, session_id);
    write_terminal_signal_marker(stderr, &payload)
}

fn terminal_signal_marker_payload(
    signal: &TerminalSignal,
    invocation_id: &Uuid,
    session_id: Option<&Uuid>,
) -> serde_json::Value {
    serde_json::json!({
        "kind": format!("{:?}", signal.kind),
        "evidence": {
            "excerpt": signal.evidence.as_str(),
        },
        "invocation_id": invocation_id.to_string(),
        "session_id": session_id.map(Uuid::to_string),
    })
}

fn write_terminal_signal_marker(
    stderr: &mut impl io::Write,
    payload: &serde_json::Value,
) -> io::Result<()> {
    writeln!(
        stderr,
        "OULIPOLY_TERMINAL_SIGNAL={}",
        serde_json::to_string(&payload).map_err(io::Error::other)?
    )
}
