//! One unprivileged per-root supervisor **process** that owns its ACP v2
//! harness subprocesses and delivers the messages it originates to them
//! through [`oulipoly_acp::AcpClient`]. Linux only.
//!
//! # What this is
//!
//! A real launchable OS process (the `oulipoly-root-supervisor` binary) that
//! launches, owns and reaps every harness of one root concurrently: one
//! process per root owning all its harnesses, never one wrapper per agent.
//! It is the first process seed of the per-root supervisor. It is
//! **unwired**: nothing on the installed route launches it, and it touches
//! no Broker State, opcode, admission record, guardian, driver or Bash
//! path. Its owned harnesses in this crate's tests are the deterministic
//! peer binary, not a real harness.
//!
//! Absorption target: a later live-placement slice makes this process the
//! root's harness-delivery owner, replacing the resume-plus-prompt path for
//! the harnesses it owns. After the store choice and the custody-invariant
//! check, per-root privileged custody and the guardian/driver and shared
//! Broker registry duties move into this lineage. If that lineage is
//! abandoned, this crate is deleted rather than kept as a fallback.
//!
//! # Interface
//!
//! The first stdin line is a JSON [`Spec`]. Later stdin lines are control
//! commands; `{"cmd":"cancel"}` is the only one. Stdin EOF is not a cancel.
//! Stdout carries one JSON object per line: progress events, then exactly
//! one `"event":"terminal"` report. Exit codes: [`EXIT_ENDED`],
//! [`EXIT_ENDED_OWED`], [`EXIT_CANCELLED`], [`EXIT_SPEC_REFUSED`].
//!
//! A missing terminal report (this process died) means every message it
//! owed is **unknown / owed lost**. Nothing is persisted, so a new instance
//! starts empty and claims no continuity with an earlier one.
//!
//! # Labels
//!
//! * `accepted` / `duplicate-unknown`: an insertion acknowledgement from the
//!   harness, with or without an at-most-once basis. Insertion only: not
//!   turn completion, not drain, and never an end condition by itself.
//! * `retained-undelivered`: still owed when the harness stopped, with a
//!   reason: `cancelled` (explicit cancel), `outage` (the closure cap was
//!   reached), `rejected`, `not-negotiated:*`, `session-*`,
//!   `invalid-response`, `launch-failed` or `not-attempted`. None of these is
//!   delivery or closure.
//! * A **closure** is a no-acknowledgement end of an attempt whose harness
//!   exit this process observed by reaping that exact child. Only closures
//!   count toward [`Spec::outage_closure_cap`]. Rejections and negotiation
//!   failures keep their own labels and are not closures. A send fault alone
//!   is labelled `send-fault`, never `PeerGone`; it becomes a closure only
//!   once the harness's exit is observed. Silence is never a closure: a quiet
//!   live harness stays owed and in flight until it answers, exits, or the
//!   caller cancels. No timer ends or kills anything.
//! * After a closure the harness is relaunched, the session resumed, and the
//!   **same** message (same key) submitted again.
//! * `idle` events are readiness since the session's first tracked attempt,
//!   not completion of the latest message.
//!
//! # Ending
//!
//! * `ended`: every owned harness exited and was reaped by exact child
//!   identity, and nothing is owed.
//! * `ended-owed`: every owned harness exited and was reaped, and some
//!   messages are retained undelivered (an outage, for example).
//! * `cancelled`: the caller cancelled. Each live owned harness is sent
//!   `SIGKILL` through a pidfd taken at spawn, its exit is observed, and
//!   every owed message is reported `retained-undelivered` / `cancelled`.
//!
//! No root PID1 terminal or Broker settlement is visible here, and neither
//! is inferred.
//!
//! # Known gaps
//!
//! * `AcpClient` keeps every session event in an unbounded `Vec` for the
//!   life of each connection. Stdout lines from a harness are bounded
//!   ([`MAX_LINE_BYTES`]).
//! * The internal event channel is unbounded.
//! * If this process dies, its harnesses are not signalled by it (the test
//!   peer arranges its own parent-death signal).
//! * In-memory only: no recovery after this process restarts.

mod harness;
mod pidfd;
mod transport;

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use serde::Deserialize;
use serde_json::{Value, json};

pub use harness::{HarnessRecord, MessageRecord};
pub use transport::MAX_LINE_BYTES;

/// Every owned harness exited and was reaped; nothing is owed.
pub const EXIT_ENDED: u8 = 0;
/// The caller cancelled; owed messages are retained undelivered.
pub const EXIT_CANCELLED: u8 = 2;
/// Every owned harness exited and was reaped, but messages are still owed.
pub const EXIT_ENDED_OWED: u8 = 3;
/// The spec line was missing or invalid; nothing was launched.
pub const EXIT_SPEC_REFUSED: u8 = 64;

/// What one supervisor run owns.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Observed no-acknowledgement closures of one message before it is
    /// labelled an outage. Must be at least 1. Counts closures, not time.
    pub outage_closure_cap: u32,
    /// Absolute working directory given to `session/new` and `session/resume`.
    pub cwd: String,
    pub harnesses: Vec<HarnessSpec>,
}

/// One owned harness: how to launch it and what to deliver to it, in order.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessSpec {
    /// Label used in reports.
    pub id: String,
    /// Program and arguments. The harness speaks ACP v2 over stdio.
    pub argv: Vec<String>,
    pub messages: Vec<String>,
}

impl Spec {
    fn validate(&self) -> Result<(), String> {
        if self.outage_closure_cap == 0 {
            return Err("outage_closure_cap must be at least 1".to_owned());
        }
        if !self.cwd.starts_with('/') {
            return Err("cwd must be absolute".to_owned());
        }
        if self.harnesses.is_empty() {
            return Err("at least one harness is required".to_owned());
        }
        let mut ids = HashSet::new();
        for harness in &self.harnesses {
            if harness.argv.is_empty() {
                return Err(format!("harness {}: argv is empty", harness.id));
            }
            if !ids.insert(harness.id.as_str()) {
                return Err(format!("duplicate harness id {}", harness.id));
            }
        }
        Ok(())
    }
}

/// What workers and the control reader tell the owning loop.
pub(crate) enum Event {
    Report(Value),
    Done(HarnessRecord),
    Control(String),
}

/// Runs one supervisor to its terminal report and returns the exit code.
pub fn run<R, W>(mut input: R, mut out: W) -> u8
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let mut first = String::new();
    let spec = match input.read_line(&mut first) {
        Ok(0) | Err(_) => Err("no spec line".to_owned()),
        Ok(_) => serde_json::from_str::<Spec>(first.trim()).map_err(|error| error.to_string()),
    }
    .and_then(|spec| spec.validate().map(|()| spec));
    let spec = match spec {
        Ok(spec) => spec,
        Err(reason) => {
            emit(
                &mut out,
                &json!({ "event": "terminal", "status": "spec-refused", "reason": reason }),
            );
            return EXIT_SPEC_REFUSED;
        }
    };

    let custody = Arc::new(Mutex::new(pidfd::Custody::default()));
    let (tx, rx) = mpsc::channel();

    let control_tx = tx.clone();
    thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { return };
            if control_tx.send(Event::Control(line)).is_err() {
                return;
            }
        }
    });

    emit(
        &mut out,
        &json!({
            "event": "started",
            "pid": std::process::id(),
            "harnesses": spec.harnesses.len(),
            "outage_closure_cap": spec.outage_closure_cap,
        }),
    );
    for harness in spec.harnesses.clone() {
        let tx = tx.clone();
        let custody = Arc::clone(&custody);
        let cap = spec.outage_closure_cap;
        let cwd = spec.cwd.clone();
        thread::spawn(move || harness::run(harness, cap, cwd, custody, tx));
    }
    drop(tx);

    let mut records = Vec::new();
    let mut cancel_requested = false;
    while records.len() < spec.harnesses.len() {
        let Ok(event) = rx.recv() else { break };
        match event {
            Event::Report(value) => emit(&mut out, &value),
            Event::Done(record) => records.push(record),
            Event::Control(line) => {
                let command = serde_json::from_str::<Value>(line.trim()).ok();
                if command
                    .as_ref()
                    .and_then(|value| value.get("cmd"))
                    .and_then(Value::as_str)
                    == Some("cancel")
                {
                    if !cancel_requested {
                        cancel_requested = true;
                        let signalled = custody.lock().expect("custody lock").cancel();
                        emit(
                            &mut out,
                            &json!({ "event": "cancel-requested", "signalled": signalled }),
                        );
                    }
                } else {
                    emit(&mut out, &json!({ "event": "control-refused" }));
                }
            }
        }
    }
    records.sort_by_key(|record| {
        spec.harnesses
            .iter()
            .position(|harness| harness.id == record.id)
    });

    let owed = records
        .iter()
        .flat_map(|record| &record.messages)
        .filter(|message| message.owed)
        .count();
    let (status, code) = if cancel_requested {
        ("cancelled", EXIT_CANCELLED)
    } else if owed > 0 {
        ("ended-owed", EXIT_ENDED_OWED)
    } else {
        ("ended", EXIT_ENDED)
    };
    emit(
        &mut out,
        &json!({
            "event": "terminal",
            "status": status,
            "owed": owed,
            "all_harnesses_reaped": records.len() == spec.harnesses.len(),
            "harnesses": records.iter().map(HarnessRecord::to_json).collect::<Vec<_>>(),
        }),
    );
    code
}

fn emit<W: Write>(out: &mut W, value: &Value) {
    // A caller that stopped reading cannot be told anything; the owning
    // loop continues so that owned children are still reaped.
    let _ = writeln!(out, "{value}").and_then(|()| out.flush());
}
