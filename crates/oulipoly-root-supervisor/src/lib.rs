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
//! [`EXIT_ENDED_OWED`], [`EXIT_CANCELLED`], [`EXIT_INCOMPLETE`],
//! [`EXIT_SPEC_REFUSED`].
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
//! * Undelivered messages and their private keys/history are retained only
//!   in the live worker's memory. At terminal process exit the owner is lost:
//!   each owed message is `undelivered-owner-lost`, with its original reason
//!   (`cancelled`, `outage`, `rejected`, `not-negotiated:*`, `session-*`,
//!   `invalid-response`, `launch-failed`, `wait-unproven`, `not-attempted`).
//!   This is neither delivery, closure of debt, nor recoverable retention.
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
//! * `ended-owed`: every actually spawned direct child exited and was
//!   successfully reaped, no further attempt is authorized, and debt remains.
//!   Exit 3 labels loss of the in-memory owner/history, not successful drain.
//! * `cancelled`: explicit cancellation is visible at run level. Admitted
//!   live children are sent `SIGKILL` by pidfd; successful waits alone prove
//!   exits/reaping. Earlier refusal/outage causes survive cancellation.
//!   Owed history is lost at process exit, with no restart continuity.
//! * `incomplete`: records or successful exit/reaping observations are
//!   missing (exit 4). A failed wait reports `wait-failed` / `unproven`,
//!   never an exit, closure count or relaunch authorization. Even a cancelled
//!   terminal may have `all_harnesses_reaped:false`; cancellation is not proof.
//! * `launches` counts actual OS spawns, including custody-open failures.
//!   Before admission, a failed custody acquisition permits exact owned-child
//!   launch cleanup; its signal result and wait observation are separate.
//! * `records_complete` checks every configured harness and message record;
//!   `all_harnesses_reaped` also requires one successful wait per spawn and no
//!   failed wait. Incomplete records report `owed:null` and `known_owed` as a
//!   partial count; `owed_history:unknown-lost-at-process-exit` is no zero-debt
//!   claim. Complete records use `lost-at-process-exit` or `nothing-owed`.
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
//! * Exit observation waits for protocol-read progress; a descendant holding
//!   stdout can delay it. A caller that does not drain output can delay cancel.
//!   Neither topology is repaired or exercised by this direct-peer slice.

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
/// The caller cancelled; owed in-memory owner/history is lost at exit.
pub const EXIT_CANCELLED: u8 = 2;
/// All spawned direct children were reaped; owed owner/history is lost at exit.
pub const EXIT_ENDED_OWED: u8 = 3;
/// Worker records or successful exit/reaping observations are incomplete.
pub const EXIT_INCOMPLETE: u8 = 4;
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

    let (report, code) = terminal_report(&spec, &records, cancel_requested);
    emit(&mut out, &report);
    code
}

fn terminal_report(spec: &Spec, records: &[HarnessRecord], cancel_requested: bool) -> (Value, u8) {
    let records_complete = records.len() == spec.harnesses.len()
        && spec.harnesses.iter().all(|harness| {
            let mut matches = records.iter().filter(|record| record.id == harness.id);
            let Some(record) = matches.next() else {
                return false;
            };
            matches.next().is_none()
                && record.messages.len() == harness.messages.len()
                && record
                    .messages
                    .iter()
                    .enumerate()
                    .all(|(index, message)| message.index == index)
        });
    let all_reaped = records_complete
        && records.iter().all(|record| {
            record.wait_failures.is_empty()
                && usize::try_from(record.launches).ok() == Some(record.exits.len())
        });
    let known_owed = records
        .iter()
        .flat_map(|record| &record.messages)
        .filter(|message| message.owed)
        .count();
    let (status, code) = if cancel_requested {
        ("cancelled", EXIT_CANCELLED)
    } else if !all_reaped {
        ("incomplete", EXIT_INCOMPLETE)
    } else if known_owed > 0 {
        ("ended-owed", EXIT_ENDED_OWED)
    } else {
        ("ended", EXIT_ENDED)
    };
    let report = json!({
        "event": "terminal",
        "status": status,
        "cancel_requested": cancel_requested,
        "owed": records_complete.then_some(known_owed),
        "known_owed": known_owed,
        "owed_history": if !records_complete { "unknown-lost-at-process-exit" }
            else if known_owed > 0 { "lost-at-process-exit" } else { "nothing-owed" },
        "records_complete": records_complete,
        "all_harnesses_reaped": all_reaped,
        "harnesses": records.iter().map(HarnessRecord::to_json).collect::<Vec<_>>(),
    });
    (report, code)
}

fn emit<W: Write>(out: &mut W, value: &Value) {
    // A caller that stopped reading cannot be told anything; the owning
    // loop continues so that owned children are still reaped.
    let _ = writeln!(out, "{value}").and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec {
            outage_closure_cap: 3,
            cwd: "/".into(),
            harnesses: vec![HarnessSpec {
                id: "test".into(),
                argv: vec!["peer".into()],
                messages: vec![],
            }],
        }
    }

    fn record() -> HarnessRecord {
        HarnessRecord {
            id: "test".into(),
            launches: 1,
            exits: vec![],
            wait_failures: vec![],
            messages: vec![],
        }
    }

    #[test]
    fn terminal_done_without_reap_is_not_normal_or_complete() {
        let (report, code) = terminal_report(&spec(), &[record()], false);
        assert_ne!(code, EXIT_ENDED);
        assert_ne!(report["status"], "ended");
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    #[test]
    fn missing_record_is_unknown_not_zero_debt_normal_end() {
        let (report, code) = terminal_report(&spec(), &[], false);
        assert_ne!(code, EXIT_ENDED);
        assert_ne!(report["status"], "ended");
        assert_eq!(report["records_complete"], false);
        assert!(report["owed"].is_null());
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    #[test]
    fn missing_message_record_is_not_positive_completeness() {
        let mut spec = spec();
        spec.harnesses[0].messages.push("x".into());
        let mut record = record();
        record.exits.push("code:0".into());
        let (report, code) = terminal_report(&spec, &[record], false);
        assert_ne!(code, EXIT_ENDED);
        assert_eq!(report["records_complete"], false);
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    #[test]
    fn terminal_debt_labels_owner_history_loss_including_cancel() {
        let mut spec = spec();
        spec.harnesses[0].messages.push("x".into());
        let mut record = record();
        record.exits.push("code:0".into());
        record.messages.push(MessageRecord {
            index: 0,
            owed: true,
            label: "outage".into(),
            at_most_once: false,
            basis: None,
            recovered: false,
            unacknowledged_attempts: 1,
            closures: 1,
        });
        for cancelled in [false, true] {
            let (report, code) = terminal_report(&spec, &[record.clone()], cancelled);
            assert_eq!(
                code,
                if cancelled {
                    EXIT_CANCELLED
                } else {
                    EXIT_ENDED_OWED
                }
            );
            assert_eq!(
                report["harnesses"][0]["messages"][0]["state"],
                "undelivered-owner-lost"
            );
            assert_eq!(report["harnesses"][0]["messages"][0]["label"], "outage");
            assert_eq!(report["owed_history"], "lost-at-process-exit");
        }
    }
}
