//! One unprivileged per-root supervisor **process** that owns its ACP v2
//! harness subprocesses, delivers the messages it originates to them
//! through [`oulipoly_acp::AcpClient`], and keeps that root's delivery
//! intent in its own private durable store so that a restarted instance
//! continues what an earlier one owed. Linux only.
//!
//! # What this is
//!
//! A real launchable OS process (the `oulipoly-root-supervisor` binary) that
//! launches, owns and reaps every harness of one root concurrently: one
//! process per root owning all its harnesses, never one wrapper per agent.
//! It is **unwired**: nothing on the installed route launches it, and it
//! touches no Broker State, opcode, admission record, global debt or fence,
//! guardian, driver or Bash path. Its owned harnesses in this crate's tests
//! are the deterministic peer binary, not a real harness.
//!
//! Absorption target: a later live-placement slice makes this process the
//! root's harness-delivery owner, replacing the resume-plus-prompt path for
//! the harnesses it owns. After the global-invariant decision and privileged
//! per-root custody, the guardian/driver and shared Broker registry duties
//! move into this lineage. If that lineage is abandoned, this crate is
//! deleted rather than kept as a fallback.
//!
//! # Interface
//!
//! The first stdin line is a JSON [`Request`]: `{"store": dir, "intent":
//! {...}}` creates a root's intent in a new store, and `{"store": dir}`
//! recovers an existing one. Later stdin lines are control commands;
//! `{"cmd":"cancel"}` is the only one. Stdin EOF is not a cancel. Stdout
//! carries one JSON object per line: progress events, then exactly one
//! `"event":"terminal"` report. Exit codes: [`EXIT_ENDED`],
//! [`EXIT_ENDED_OWED`], [`EXIT_CANCELLED`], [`EXIT_INCOMPLETE`],
//! [`EXIT_STORE_LOST`], [`EXIT_SPEC_REFUSED`], [`EXIT_STORE_REFUSED`].
//!
//! # Durable store and ownership
//!
//! `store` is one root's private directory (created `0700`; an existing one
//! must be owned by this user and not group/other accessible). It holds an
//! owner lock file and one SQLite database; see the `store` module. One
//! store per root: nothing in it is shared with another root, and global
//! coordination stays where it is.
//!
//! * A second live instance for the same store is refused (`owner-live`)
//!   before it opens the database.
//! * Each instance claims a durable owner generation. Every later write
//!   first checks it is still the latest; a stale instance is refused
//!   (`authority-lost`) before changing anything, and then stops its own
//!   harnesses rather than deliver what it can no longer record.
//! * The premise is a cooperative local administrator. File modes, the lock
//!   and the generation answer accidental stale or duplicate instances; they
//!   do not prove that no other process can write the file.
//!
//! # Five stages, kept distinct
//!
//! 1. **Accepted by interface**: `intent-received`, emitted after the request
//!    is parsed and validated and before anything is written. Not durable.
//! 2. **Durably committed**: `intent-committed`, emitted only after the
//!    transaction holding the intent and every minted message key has
//!    committed. A crash before that leaves no intent; a recover request
//!    then reports `no-durable-intent`. Durability target: survives this
//!    process's crash and an OS/host crash. Means: WAL journal with
//!    `synchronous=FULL` (each commit syncs the WAL before returning), plus
//!    an fsync of the store directory after the claim. Host-crash survival
//!    also assumes the storage honours fsync; that is not tested here.
//!    There is no batching: each transition is its own commit.
//! 3. **Attempted delivery**: an attempt row is committed **before** each
//!    `session/prompt` is sent.
//! 4. **Consumption ACK**: the insertion acknowledgement is committed before
//!    its `ack` event. An ACK received but not recorded is not reported.
//! 5. **Transition completion**: never observed (the ACP draft has no
//!    completion correlation); reported as `completion:"not-observed"`.
//!
//! # Restart recovery
//!
//! A recovering instance takes the next owner generation and, in the same
//! transaction, labels every earlier-generation attempt that has no recorded
//! outcome `unknown-prior-owner`: it may or may not have been sent or
//! inserted. That is neither an acknowledgement nor a closure: the earlier
//! owner's death is not an observed harness exit. It then resumes the
//! recorded session and resubmits each owed message with its **original**
//! key. A restored key is a supplied identity with unknown history, so an
//! ACK after a restart is at best `duplicate-unknown`, with `recovered`
//! when a dedup-contract receiver says it returned an earlier insertion.
//! Durable storage does not make a restored key or session string a
//! receiver-continuity proof.
//!
//! Attempt counts, closures and the `outage` / `attempts-exhausted` stops
//! persist, so a restart never resets either cap. They are separate facts:
//! a closure is an observed harness exit, while an attempt is any recorded
//! send, including ones whose outcome is unknown because their owner died.
//! [`Intent::delivery_attempt_cap`] bounds attempts per message over every
//! generation, so a loop of owner restarts with unresolved outcomes cannot
//! retry forever; when it is used up the message stops as
//! `attempts-exhausted` with its unknown attempts still unknown, not as an
//! outage, a closure or an acknowledgement. Harnesses launched by a dead instance are not
//! re-adopted: recovering processes that outlived their owner needs custody
//! this unprivileged process does not have. Recovery relaunches under the
//! same keys instead.
//!
//! # Labels
//!
//! * `accepted` / `duplicate-unknown`: a durably recorded insertion
//!   acknowledgement from the harness, with or without an at-most-once
//!   basis. Insertion only: not turn completion, not drain, and never an end
//!   condition by itself.
//! * An owed message keeps its reason: `cancelled`, `outage`, `rejected`,
//!   `not-negotiated:*`, `session-*`, `invalid-response`, `launch-failed`,
//!   `wait-unproven`, `authority-lost`, `store-failed`, `not-attempted`,
//!   `attempts-exhausted`. Only `outage` and `attempts-exhausted` are durable
//!   stops; the others end this instance's attempts and a later recovery
//!   retries the message.
//! * A **closure** is a no-acknowledgement end of an attempt whose harness
//!   exit this process observed by reaping that exact child. Only closures
//!   count toward [`Intent::outage_closure_cap`]. Rejections and negotiation
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
//!   successfully reaped, no further attempt is authorized in this instance,
//!   and debt remains, retained in the store (exit 3).
//! * `cancelled`: explicit cancellation is visible at run level. Admitted
//!   live children are sent `SIGKILL` by pidfd; successful waits alone prove
//!   exits/reaping. Earlier refusal/outage causes survive cancellation.
//!   Cancel ends this instance; it does not withdraw durable intent.
//! * `authority-lost` / `store-failed`: a store write was refused or failed
//!   (exit 5). Own harnesses are signalled; what the store holds is
//!   authoritative, this instance's view is not.
//! * `incomplete`: records or successful exit/reaping observations are
//!   missing (exit 4). A failed wait reports `wait-failed` / `unproven`,
//!   never an exit, closure count or relaunch authorization. Even a cancelled
//!   terminal may have `all_harnesses_reaped:false`; cancellation is not proof.
//! * `launches` counts this instance's actual OS spawns, including
//!   custody-open failures.
//! * `records_complete` checks every durable harness and message record;
//!   `all_harnesses_reaped` also requires one successful wait per spawn and no
//!   failed wait. Incomplete records report `owed:null` and `known_owed` as a
//!   partial count.
//!
//! No root PID1 terminal or Broker settlement is visible here, and neither
//! is inferred. A missing terminal report (this process died) says nothing
//! about delivery; the store says what was recorded.
//!
//! # Known gaps
//!
//! * `AcpClient` keeps every session event in an unbounded `Vec` for the
//!   life of each connection. Stdout lines from a harness are bounded
//!   ([`MAX_LINE_BYTES`]).
//! * The internal event channel is unbounded.
//! * If this process dies, its harnesses are not signalled by it (the test
//!   peer arranges its own parent-death signal), and a successor does not
//!   re-adopt them.
//! * Between an attempt's commit and its send, a successor that claimed in
//!   that window could resend the same key concurrently; labels stay honest
//!   (`duplicate-unknown`) but a non-dedup receiver may insert twice.
//! * Store growth and retention are unbounded; nothing is pruned.
//! * Exit observation waits for protocol-read progress; a descendant holding
//!   stdout can delay it. A caller that does not drain output can delay cancel.

mod harness;
mod pidfd;
mod store;
mod transport;

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use serde::Deserialize;
use serde_json::{Value, json};

pub use harness::{HarnessRecord, MessageRecord};
pub use transport::MAX_LINE_BYTES;

/// Every owned harness exited and was reaped; nothing is owed.
pub const EXIT_ENDED: u8 = 0;
/// The caller cancelled; owed messages stay in the store for a recovery.
pub const EXIT_CANCELLED: u8 = 2;
/// All spawned direct children were reaped; owed messages stay in the store.
pub const EXIT_ENDED_OWED: u8 = 3;
/// Worker records or successful exit/reaping observations are incomplete.
pub const EXIT_INCOMPLETE: u8 = 4;
/// A store write was refused (stale owner) or failed; own harnesses were
/// signalled.
pub const EXIT_STORE_LOST: u8 = 5;
/// The request line was missing or invalid; nothing was written or launched.
pub const EXIT_SPEC_REFUSED: u8 = 64;
/// The store could not be claimed (live owner, missing or existing intent,
/// not private, or a store error); nothing was launched.
pub const EXIT_STORE_REFUSED: u8 = 65;

/// The first stdin line.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Absolute path of this root's private store directory.
    pub store: String,
    /// Present to create the root's intent; absent to recover it.
    pub intent: Option<Intent>,
}

/// What one root's supervisor owes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    /// Observed no-acknowledgement closures of one message before it is
    /// labelled an outage. Must be at least 1. Counts closures, not time.
    pub outage_closure_cap: u32,
    /// Recorded delivery attempts of one message, over every owner
    /// generation, before it stops as `attempts-exhausted`. Must be at least
    /// 1. Counts attempts whatever their outcome, including unknown ones.
    pub delivery_attempt_cap: u32,
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

impl Request {
    fn validate(&self) -> Result<(), String> {
        if !self.store.starts_with('/') {
            return Err("store must be absolute".to_owned());
        }
        self.intent.as_ref().map_or(Ok(()), Intent::validate)
    }
}

impl Intent {
    fn validate(&self) -> Result<(), String> {
        if self.outage_closure_cap == 0 {
            return Err("outage_closure_cap must be at least 1".to_owned());
        }
        if self.delivery_attempt_cap == 0 {
            return Err("delivery_attempt_cap must be at least 1".to_owned());
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
    let request = match input.read_line(&mut first) {
        Ok(0) | Err(_) => Err("no request line".to_owned()),
        Ok(_) => serde_json::from_str::<Request>(first.trim()).map_err(|error| error.to_string()),
    }
    .and_then(|request| request.validate().map(|()| request));
    let request = match request {
        Ok(request) => request,
        Err(reason) => {
            emit(
                &mut out,
                &json!({ "event": "terminal", "status": "spec-refused", "reason": reason }),
            );
            return EXIT_SPEC_REFUSED;
        }
    };
    if request.intent.is_some() {
        emit(
            &mut out,
            &json!({ "event": "intent-received", "stage": "accepted-by-interface", "durable": false }),
        );
    }
    let claimed = match store::Store::claim(Path::new(&request.store), request.intent.as_ref()) {
        Ok(claimed) => claimed,
        Err(error) => {
            emit(
                &mut out,
                &json!({ "event": "terminal", "status": "store-refused", "reason": error.reason() }),
            );
            return EXIT_STORE_REFUSED;
        }
    };
    let generation = claimed.store.generation();
    if claimed.created {
        emit(
            &mut out,
            &json!({
                "event": "intent-committed",
                "stage": "durably-committed",
                "durability": "sqlite-wal-synchronous-full",
                "generation": generation,
            }),
        );
    } else {
        emit(
            &mut out,
            &json!({
                "event": "intent-recovered",
                "generation": generation,
                "prior_attempts_unknown": claimed.classified_unknown,
            }),
        );
    }
    let expected: Vec<(String, usize)> = claimed
        .harnesses
        .iter()
        .map(|harness| (harness.id.clone(), harness.messages.len()))
        .collect();

    let custody = Arc::new(Mutex::new(pidfd::Custody::default()));
    let store = Arc::new(Mutex::new(claimed.store));
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
            "generation": generation,
            "harnesses": expected.len(),
            "outage_closure_cap": claimed.outage_closure_cap,
            "delivery_attempt_cap": claimed.delivery_attempt_cap,
        }),
    );
    for (position, harness) in claimed.harnesses.into_iter().enumerate() {
        let assignment = harness::Assignment {
            position,
            harness,
            cap: claimed.outage_closure_cap,
            attempt_cap: claimed.delivery_attempt_cap,
            cwd: claimed.cwd.clone(),
            custody: Arc::clone(&custody),
            store: Arc::clone(&store),
            tx: tx.clone(),
        };
        thread::spawn(move || harness::run(assignment));
    }
    drop(tx);

    let mut records = Vec::new();
    let mut cancel_requested = false;
    while records.len() < expected.len() {
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
    records.sort_by_key(|record| expected.iter().position(|(id, _)| *id == record.id));

    let stop = custody.lock().expect("custody lock").reason();
    let store_lost = matches!(stop, Some("authority-lost" | "store-failed"))
        .then_some(stop)
        .flatten();
    let (report, code) = terminal_report(&expected, &records, cancel_requested, store_lost);
    emit(&mut out, &report);
    code
}

fn terminal_report(
    expected: &[(String, usize)],
    records: &[HarnessRecord],
    cancel_requested: bool,
    store_lost: Option<&str>,
) -> (Value, u8) {
    let records_complete = records.len() == expected.len()
        && expected.iter().all(|(id, messages)| {
            let mut matches = records.iter().filter(|record| record.id == *id);
            let Some(record) = matches.next() else {
                return false;
            };
            matches.next().is_none()
                && record.messages.len() == *messages
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
    let (status, code) = if let Some(lost) = store_lost {
        (lost, EXIT_STORE_LOST)
    } else if cancel_requested {
        ("cancelled", EXIT_CANCELLED)
    } else if !all_reaped {
        ("incomplete", EXIT_INCOMPLETE)
    } else if known_owed > 0 {
        ("ended-owed", EXIT_ENDED_OWED)
    } else {
        ("ended", EXIT_ENDED)
    };
    let owed_history = if store_lost.is_some() {
        "store-holds-authoritative-state"
    } else if !records_complete {
        "unknown-partial-records"
    } else if known_owed > 0 {
        "retained-in-store"
    } else {
        "nothing-owed"
    };
    let report = json!({
        "event": "terminal",
        "status": status,
        "cancel_requested": cancel_requested,
        "owed": records_complete.then_some(known_owed),
        "known_owed": known_owed,
        "owed_history": owed_history,
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

    fn expected(messages: usize) -> Vec<(String, usize)> {
        vec![("test".into(), messages)]
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
        let (report, code) = terminal_report(&expected(0), &[record()], false, None);
        assert_ne!(code, EXIT_ENDED);
        assert_ne!(report["status"], "ended");
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    #[test]
    fn missing_record_is_unknown_not_zero_debt_normal_end() {
        let (report, code) = terminal_report(&expected(0), &[], false, None);
        assert_ne!(code, EXIT_ENDED);
        assert_ne!(report["status"], "ended");
        assert_eq!(report["records_complete"], false);
        assert!(report["owed"].is_null());
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    #[test]
    fn missing_message_record_is_not_positive_completeness() {
        let mut record = record();
        record.exits.push("code:0".into());
        let (report, code) = terminal_report(&expected(1), &[record], false, None);
        assert_ne!(code, EXIT_ENDED);
        assert_eq!(report["records_complete"], false);
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    fn owed_record() -> HarnessRecord {
        let mut record = record();
        record.exits.push("code:0".into());
        record.messages.push(MessageRecord {
            index: 0,
            owed: true,
            label: "outage".into(),
            at_most_once: false,
            basis: None,
            recovered: false,
            ack_generation: None,
            attempts: 1,
            prior_unknown: 0,
            closures: 1,
        });
        record
    }

    #[test]
    fn terminal_debt_is_retained_in_store_including_cancel() {
        for cancelled in [false, true] {
            let (report, code) = terminal_report(&expected(1), &[owed_record()], cancelled, None);
            assert_eq!(
                code,
                if cancelled {
                    EXIT_CANCELLED
                } else {
                    EXIT_ENDED_OWED
                }
            );
            assert_eq!(report["harnesses"][0]["messages"][0]["state"], "owed");
            assert_eq!(report["harnesses"][0]["messages"][0]["label"], "outage");
            assert_eq!(
                report["harnesses"][0]["messages"][0]["completion"],
                "not-observed"
            );
            assert_eq!(report["owed_history"], "retained-in-store");
        }
    }

    #[test]
    fn lost_store_authority_outranks_cancel_and_claims_no_retention() {
        let (report, code) =
            terminal_report(&expected(1), &[owed_record()], true, Some("authority-lost"));
        assert_eq!(code, EXIT_STORE_LOST);
        assert_eq!(report["status"], "authority-lost");
        assert_eq!(report["owed_history"], "store-holds-authoritative-state");
    }
}
