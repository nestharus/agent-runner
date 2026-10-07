//! The root's control face in the shared vocabulary
//! (`oulipoly.session_control/v2`, from `agent-provider-contract`). This
//! owner is the root authority the vocabulary addresses; the contract
//! defines what the claims mean and refuses ones that contradict, and this
//! module realizes them: authority, admission, enforcement, durability and
//! the evidence behind settlement readings.
//!
//! # Authority and requester
//!
//! The owner's authority is `{root, owner, generation, incarnation}`: the
//! store's `root_id`, this claim's random owner token, its owner generation
//! and the root PID 1 incarnation it holds (`none` before one is recorded).
//! A request addressed to any other authority of this root is refused
//! `stale_authority`, naming this one. Values are compared for equality
//! only; the generation fence, not the value, is what stops a stale owner.
//!
//! The root's requester is `uid:<n>`, the uid its work runs as (the
//! requester the front door attested). Only it may address a request here
//! (`not_permitted` otherwise). The stdin channel carrying controls is the
//! attestation: whoever can write it is trusted as that requester. The
//! existing stdin shorthand `{"cmd":"close"}` enters the same ladder as a
//! `close` request of requester `stdin` with a key from its control number,
//! so one close meaning exists whatever carried it. `{"cmd":"cancel"}`
//! keeps its own, different meaning: this owner instance's cancellation,
//! after which a later recovery still owes the root's work. The root's
//! durable lifecycle cancel is only the `cancel` operation here.
//!
//! # The ladder, durably
//!
//! A request this owner can answer is committed with its receipt and
//! admission before they are reported; the acknowledgment (or transition
//! refusal) and the acknowledged outcome are committed together before the
//! transition is applied and reported. Each record is kept as its exact
//! line. A repeated identical request is answered by replaying those lines
//! unchanged and makes no second transition; the same key with other
//! content is refused `key_conflict` (not kept). A store failure leaves an
//! `unknown` outcome (`evidence_unavailable`), not kept, and no transition.
//!
//! A later owner reads the kept claims back as durable control intent: an
//! acknowledged hold stays held, an acknowledged close keeps input closed
//! and is followed through, and an acknowledged cancel cancels. An
//! acknowledgment never becomes absence after owner death. A request an
//! earlier owner admitted but never answered stays pending; the successor
//! cannot acknowledge it, reports `unknown` (`authority_changed`) with
//! itself as reporter, and needs a new request addressed to it.
//!
//! # Operations here
//!
//! * `input_hold` / `input_release`: offered together. A hold refuses new
//!   caller input only ([`crate::conversation::InputHold`]); running turns,
//!   tools, Bash completions, close and cancel are unchanged.
//! * `close`: the crate's close (input closed root-wide, harnesses stopped
//!   after tagged turn ends). After a cancel it is refused `already_terminal`.
//! * `cancel`: the crate's cancel; outranks close.
//! * `recover`: answered only by a recovering owner of the same root and
//!   incarnation, from the recover request's `control` record: acknowledged
//!   `attached` after a positive attach, refused `root_absent` when no root
//!   PID 1 is found, `transition_failed` when it may run but could not be
//!   attached. A live owner refuses it `owner_live` (not kept).
//! * Requests scoped to a child are refused `unsupported_operation`.
//!
//! # Inspection and settlement
//!
//! `{"cmd":"inspect"}` reports a `control_state` (input, lifecycle with the
//! request that set each, pending intents) and the root's settlement:
//! one `observation` per fact, read with [`sc::read_settlement`] under a
//! lineage of this owner alone. The facts come from the store every
//! generation wrote under the generation fence: insertion from attempt
//! outcomes and ACKs, the agent's tagged turn end, async completion debt,
//! and physical custody of every launch and incarnation. Physical custody
//! never enters the logical reading.
//!
//! **Retirement** is described as eligible only when every input reads
//! `settled` or `not_inserted` with warranted basis, no async completion is
//! owed, every launch and incarnation is recorded ended by its actual
//! waiter, and no control intent is pending. An ACK without a tagged end,
//! a physical exit, `closed`/7 or an absent ACK alone never makes a root
//! eligible. The decision to discard stays the caller's.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_provider_contract::session_control as sc;
use serde_json::{Value, json};

use crate::conversation::InputHold;
use crate::store::{ControlRow, SettlementFacts, Store};

pub(crate) use sc::{Operation, Record, State};

/// The requester of stdin shorthand controls.
const STDIN_REQUESTER: &str = "stdin";

/// What this owner offers: every operation, inspection, every fact type.
pub(crate) fn offer() -> sc::Offer {
    sc::Offer {
        operations: vec![
            sc::Operation::InputHold,
            sc::Operation::InputRelease,
            sc::Operation::Recover,
            sc::Operation::Cancel,
            sc::Operation::Close,
        ],
        reports: vec![sc::Report::Inspection],
        facts: vec![
            sc::FactType::Insertion,
            sc::FactType::TaggedEnd,
            sc::FactType::LogicalSettlement,
            sc::FactType::PhysicalCustody,
        ],
    }
}

/// A transition to apply after its acknowledgment is durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    Hold,
    Release,
    Close,
    Cancel,
}

/// How a recovering owner found the root it was asked to recover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    Attached,
    Absent,
    Unattached,
}

/// One request and every kept claim answering it.
struct Entry {
    request: sc::Request,
    trace: sc::RequestTrace,
    lines: Vec<String>,
}

pub(crate) struct Face {
    root: String,
    requester: String,
    owner: String,
    generation: i64,
    incarnation: Option<i64>,
    entries: Vec<Entry>,
    input: sc::Current,
    lifecycle: sc::Current,
    hold: Arc<InputHold>,
    /// Kept lines that could not be read back as claims.
    unreadable: usize,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn line(record: &sc::Record) -> String {
    record.encode_line()
}

fn addressed_key(authority: &sc::Authority) -> String {
    serde_json::to_string(authority).expect("authority serializes")
}

impl Face {
    /// Reads every kept claim back and derives the durable current state.
    pub(crate) fn load(
        store: &Store,
        root: &str,
        requester: &str,
        incarnation: Option<i64>,
        hold: Arc<InputHold>,
    ) -> rusqlite::Result<Self> {
        let mut face = Self {
            root: root.to_owned(),
            requester: requester.to_owned(),
            owner: store.owner_token().to_owned(),
            generation: store.generation(),
            incarnation,
            entries: Vec::new(),
            input: sc::Current {
                state: sc::State::InputOpen,
                since: None,
            },
            lifecycle: sc::Current {
                state: sc::State::Open,
                since: None,
            },
            hold,
            unreadable: 0,
        };
        for row in store.control_rows()? {
            face.restore(&row);
        }
        face.hold.set(face.input.state == sc::State::InputHeld);
        Ok(face)
    }

    fn restore(&mut self, row: &ControlRow) {
        let Ok(record) = sc::Record::decode_line(&row.line) else {
            self.unreadable += 1;
            return;
        };
        if let sc::Record::Request(request) = &record {
            match sc::RequestTrace::new(request.clone()) {
                Ok(trace) => self.entries.push(Entry {
                    request: request.clone(),
                    trace,
                    lines: vec![row.line.clone()],
                }),
                Err(_) => self.unreadable += 1,
            }
            return;
        }
        let Some(index) = self.find(&record) else {
            self.unreadable += 1;
            return;
        };
        let entry = &mut self.entries[index];
        if entry.trace.accept(&record).is_err() {
            self.unreadable += 1;
            return;
        }
        entry.lines.push(row.line.clone());
        if let sc::Record::Acknowledgment(ack) = &record {
            let reference = entry.request.reference();
            self.transition(ack.operation, ack.to, reference);
        }
    }

    fn find(&self, record: &sc::Record) -> Option<usize> {
        let (key, requester, addressed) = record.correlation()?;
        self.entries.iter().position(|entry| {
            entry.request.request_key == key
                && entry.request.requester == requester
                && &entry.request.addressed == addressed
        })
    }

    /// Records an acknowledged transition in the current state. A lifecycle
    /// state never returns to an earlier one; the first request to reach a
    /// state is the one that set it.
    fn transition(&mut self, operation: sc::Operation, to: sc::State, since: sc::RequestRef) {
        match operation.domain() {
            sc::Domain::Input => {
                if self.input.state != to {
                    self.input = sc::Current {
                        state: to,
                        since: Some(since),
                    };
                }
            }
            sc::Domain::Lifecycle => {
                let rank = |state| match state {
                    sc::State::Closing => 1,
                    sc::State::Cancelling => 2,
                    _ => 0,
                };
                if rank(to) > rank(self.lifecycle.state) {
                    self.lifecycle = sc::Current {
                        state: to,
                        since: Some(since),
                    };
                }
            }
            sc::Domain::Attachment => {}
        }
    }

    pub(crate) fn set_incarnation(&mut self, incarnation: Option<i64>) {
        self.incarnation = incarnation;
    }

    pub(crate) fn authority(&self) -> sc::Authority {
        sc::Authority {
            root: self.root.clone(),
            owner: self.owner.clone(),
            generation: self.generation.to_string(),
            incarnation: self
                .incarnation
                .map_or_else(|| "none".to_owned(), |id| id.to_string()),
        }
    }

    pub(crate) fn input_state(&self) -> sc::State {
        self.input.state
    }

    pub(crate) fn lifecycle_state(&self) -> sc::State {
        self.lifecycle.state
    }

    /// The owner's announcement: protocol, advertisement, authority, the
    /// requester it answers, and the durable state it inherited.
    pub(crate) fn announcement(&self) -> Value {
        json!({
            "event": "session-control",
            "protocol": sc::PROTOCOL,
            "advertisement": sc::advertisement(&offer()),
            "authority": self.authority(),
            "requester": self.requester,
            "input": self.input,
            "lifecycle": self.lifecycle,
            "kept_requests": self.entries.len(),
            "unreadable_records": self.unreadable,
        })
    }

    /// The current state report.
    pub(crate) fn state(&self) -> sc::ControlState {
        let pending = self
            .pending()
            .rev()
            .take(sc::MAX_PENDING)
            .map(|entry| sc::Pending {
                request: entry.request.reference(),
                operation: entry.request.operation,
                status: if entry.trace.admission().is_some() {
                    sc::PendingStatus::Admitted
                } else {
                    sc::PendingStatus::Received
                },
            })
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        sc::ControlState {
            protocol: sc::PROTOCOL.to_owned(),
            reporter: self.authority(),
            scope: sc::ControlScope {
                root: self.root.clone(),
                child: None,
            },
            input: self.input.clone(),
            lifecycle: self.lifecycle.clone(),
            pending,
            observed_at_unix_ms: now_ms(),
        }
    }

    /// Kept requests with neither acknowledgment nor refusal.
    fn pending(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter().filter(|entry| {
            entry.request.operation != sc::Operation::Recover
                && entry.trace.acknowledgment().is_none()
                && entry.trace.refusal().is_none()
        })
    }

    /// A successor's report on each request an earlier owner admitted and
    /// never answered: `unknown` (`authority_changed`), kept. Returns the
    /// lines to emit.
    pub(crate) fn report_inherited(&mut self, store: &mut Store) -> Vec<String> {
        let me = self.authority();
        let mut lines = Vec::new();
        let indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.request.operation != sc::Operation::Recover
                    && entry.trace.acknowledgment().is_none()
                    && entry.trace.refusal().is_none()
                    && entry.request.addressed != me
            })
            .map(|(index, _)| index)
            .collect();
        for index in indices {
            let request = &self.entries[index].request;
            let outcome = sc::Record::Outcome(sc::Outcome {
                protocol: sc::PROTOCOL.to_owned(),
                request_key: request.request_key.clone(),
                requester: request.requester.clone(),
                addressed: request.addressed.clone(),
                result: sc::OutcomeResult::Unknown,
                uncertainty: Some(sc::Uncertainty::AuthorityChanged),
                reporter: Some(me.clone()),
                observed_at_unix_ms: now_ms(),
            });
            let mut trace = self.entries[index].trace.clone();
            if trace.accept(&outcome).is_err() {
                continue;
            }
            let text = line(&outcome);
            if store
                .append_control(&[self.row(request, "outcome", &text)])
                .is_err()
            {
                break;
            }
            self.entries[index].trace = trace;
            self.entries[index].lines.push(text.clone());
            lines.push(text);
        }
        lines
    }

    fn row(&self, request: &sc::Request, kind: &str, text: &str) -> ControlRow {
        ControlRow {
            request_key: request.request_key.clone(),
            requester: request.requester.clone(),
            addressed: addressed_key(&request.addressed),
            kind: kind.to_owned(),
            line: text.to_owned(),
            generation: self.generation,
        }
    }

    /// The stdin shorthand `close` as a request of the stdin requester,
    /// addressed to this owner.
    pub(crate) fn shorthand(&self, operation: sc::Operation, control: u64) -> sc::Request {
        sc::Request {
            protocol: sc::PROTOCOL.to_owned(),
            request_key: format!("stdin-g{}-c{control}", self.generation),
            requester: STDIN_REQUESTER.to_owned(),
            addressed: self.authority(),
            operation,
            scope: sc::ControlScope {
                root: self.root.clone(),
                child: None,
            },
            reason: None,
        }
    }

    /// Answers one control line that is a v2 record. Returns the lines to
    /// emit (claims, or one control-only diagnostic) and the transition to
    /// apply, if its acknowledgment is now durable.
    pub(crate) fn handle_line(
        &mut self,
        text: &str,
        store: &mut Store,
    ) -> (Vec<String>, Option<Effect>) {
        let record = match sc::Record::decode_line(text) {
            Ok(record) => record,
            Err(diagnostic) => return (vec![unavailable(&diagnostic)], None),
        };
        let sc::Record::Request(request) = record else {
            let diagnostic = sc::ControlUnavailable::new(
                sc::UnavailableReason::ProtocolViolation,
                "the root owner accepts requests only",
            );
            return (vec![unavailable(&diagnostic)], None);
        };
        if request.requester != self.requester {
            let refusal = self.refusal(&request, sc::RefusalReason::NotPermitted);
            return self.answer(&request, vec![refusal], true, store);
        }
        self.handle(request, store)
    }

    /// Answers one admissible request (the shorthand enters here).
    pub(crate) fn handle(
        &mut self,
        request: sc::Request,
        store: &mut Store,
    ) -> (Vec<String>, Option<Effect>) {
        if request.addressed.root != self.root {
            let refusal = self.refusal(&request, sc::RefusalReason::UnknownScope);
            return self.answer(&request, vec![refusal], false, store);
        }
        if let Some(index) = self.entries.iter().position(|entry| {
            sc::classify_repetition(&entry.request, &request) != sc::Repetition::Distinct
        }) {
            let entry = &self.entries[index];
            return match sc::classify_repetition(&entry.request, &request) {
                // Faithful replay: the kept lines, unchanged; no transition.
                sc::Repetition::SameRequest => (entry.lines[1..].to_vec(), None),
                _ => {
                    let refusal = self.refusal(&request, sc::RefusalReason::KeyConflict);
                    self.answer(&request, vec![refusal], false, store)
                }
            };
        }
        if request.scope.child.is_some() {
            let refusal = self.refusal(&request, sc::RefusalReason::UnsupportedOperation);
            return self.answer(&request, vec![refusal], true, store);
        }
        if request.operation == sc::Operation::Recover {
            // This owner holds the root: there is nothing to recover.
            let refusal = self.refusal(&request, sc::RefusalReason::OwnerLive);
            return self.answer(&request, vec![refusal], false, store);
        }
        let me = self.authority();
        if request.addressed != me {
            let refusal = self.refusal(&request, sc::RefusalReason::StaleAuthority);
            return self.answer(&request, vec![refusal], true, store);
        }
        let (from, to, effect) = match request.operation {
            sc::Operation::InputHold => (self.input.state, sc::State::InputHeld, Effect::Hold),
            sc::Operation::InputRelease => {
                (self.input.state, sc::State::InputOpen, Effect::Release)
            }
            sc::Operation::Close => (self.lifecycle.state, sc::State::Closing, Effect::Close),
            sc::Operation::Cancel => (self.lifecycle.state, sc::State::Cancelling, Effect::Cancel),
            sc::Operation::Recover => unreachable!("answered above"),
        };
        let answer = if effect == Effect::Close && from == sc::State::Cancelling {
            Err(sc::RefusalReason::AlreadyTerminal)
        } else {
            Ok((from, to))
        };
        self.admit_and_answer(request, answer, Some(effect), store)
    }

    /// Admits `request` durably, then commits its answer durably: an
    /// acknowledgment `from` → `to`, or a transition refusal.
    fn admit_and_answer(
        &mut self,
        request: sc::Request,
        answer: Result<(sc::State, sc::State), sc::RefusalReason>,
        effect: Option<Effect>,
        store: &mut Store,
    ) -> (Vec<String>, Option<Effect>) {
        let me = self.authority();
        let receipt = sc::Record::Receipt(sc::Receipt {
            protocol: sc::PROTOCOL.to_owned(),
            request_key: request.request_key.clone(),
            requester: request.requester.clone(),
            addressed: request.addressed.clone(),
            durable: true,
            observed_at_unix_ms: now_ms(),
        });
        let admission = sc::Record::Admission(sc::Admission {
            protocol: sc::PROTOCOL.to_owned(),
            request_key: request.request_key.clone(),
            requester: request.requester.clone(),
            addressed: request.addressed.clone(),
            operation: request.operation,
            responder: me.clone(),
            observed_at_unix_ms: now_ms(),
        });
        let Ok(mut trace) = sc::RequestTrace::new(request.clone()) else {
            let diagnostic = sc::ControlUnavailable::new(
                sc::UnavailableReason::InvalidRecord,
                "request is not admissible",
            );
            return (vec![unavailable(&diagnostic)], None);
        };
        let request_line = line(&sc::Record::Request(request.clone()));
        let first = [receipt, admission];
        if first.iter().any(|record| trace.accept(record).is_err()) {
            let diagnostic = sc::ControlUnavailable::new(
                sc::UnavailableReason::ProtocolViolation,
                "admission would contradict the trace",
            );
            return (vec![unavailable(&diagnostic)], None);
        }
        let mut lines: Vec<String> = first.iter().map(line).collect();
        let rows: Vec<ControlRow> = std::iter::once(("request", &request_line))
            .chain([("receipt", &lines[0]), ("admission", &lines[1])])
            .map(|(kind, text)| self.row(&request, kind, text))
            .collect();
        if store.append_control(&rows).is_err() {
            return (vec![line(&self.unknown(&request))], None);
        }
        let mut kept = vec![request_line];
        kept.extend(lines.iter().cloned());
        let answer_record = match answer {
            Ok((from, to)) => sc::Record::Acknowledgment(sc::Acknowledgment {
                protocol: sc::PROTOCOL.to_owned(),
                request_key: request.request_key.clone(),
                requester: request.requester.clone(),
                addressed: request.addressed.clone(),
                responder: me.clone(),
                operation: request.operation,
                from,
                to,
                observed_at_unix_ms: now_ms(),
            }),
            Err(reason) => sc::Record::Refusal(sc::Refusal {
                protocol: sc::PROTOCOL.to_owned(),
                request_key: request.request_key.clone(),
                requester: request.requester.clone(),
                addressed: request.addressed.clone(),
                operation: request.operation,
                stage: reason.stage(),
                reason,
                responder: Some(me.clone()),
                detail: None,
                observed_at_unix_ms: now_ms(),
            }),
        };
        let outcome = sc::Record::Outcome(sc::Outcome {
            protocol: sc::PROTOCOL.to_owned(),
            request_key: request.request_key.clone(),
            requester: request.requester.clone(),
            addressed: request.addressed.clone(),
            result: if answer.is_ok() {
                sc::OutcomeResult::Acknowledged
            } else {
                sc::OutcomeResult::Refused
            },
            uncertainty: None,
            reporter: Some(me),
            observed_at_unix_ms: now_ms(),
        });
        let mut answered = trace.clone();
        let second = [answer_record, outcome];
        let accepted = second.iter().all(|record| answered.accept(record).is_ok());
        let second_lines: Vec<String> = second.iter().map(line).collect();
        let second_rows: Vec<ControlRow> = [
            (
                if answer.is_ok() {
                    "acknowledgment"
                } else {
                    "refusal"
                },
                &second_lines[0],
            ),
            ("outcome", &second_lines[1]),
        ]
        .into_iter()
        .map(|(kind, text)| self.row(&request, kind, text))
        .collect();
        if !accepted || store.append_control(&second_rows).is_err() {
            // Admitted and kept; what became of it is unknown here.
            let unknown = line(&self.unknown(&request));
            lines.push(unknown);
            self.entries.push(Entry {
                request,
                trace,
                lines: kept,
            });
            return (lines, None);
        }
        kept.extend(second_lines.iter().cloned());
        lines.extend(second_lines);
        let reference = request.reference();
        self.entries.push(Entry {
            request: request.clone(),
            trace: answered,
            lines: kept,
        });
        match answer {
            Ok((_, to)) => {
                self.transition(request.operation, to, reference);
                (lines, effect)
            }
            Err(_) => (lines, None),
        }
    }

    /// A recovering owner's answer to the recover request it was started
    /// with, once it knows what it found.
    pub(crate) fn answer_recover(
        &mut self,
        text: &str,
        found: Found,
        store: &mut Store,
    ) -> Vec<String> {
        let request = match sc::Record::decode_line(text) {
            Ok(sc::Record::Request(request)) if request.operation == sc::Operation::Recover => {
                request
            }
            Ok(_) => {
                let diagnostic = sc::ControlUnavailable::new(
                    sc::UnavailableReason::ProtocolViolation,
                    "a recovery's control record is a recover request",
                );
                return vec![unavailable(&diagnostic)];
            }
            Err(diagnostic) => return vec![unavailable(&diagnostic)],
        };
        if request.requester != self.requester {
            let refusal = self.refusal(&request, sc::RefusalReason::NotPermitted);
            return self.answer(&request, vec![refusal], true, store).0;
        }
        if request.addressed.root != self.root {
            let refusal = self.refusal(&request, sc::RefusalReason::UnknownScope);
            return self.answer(&request, vec![refusal], false, store).0;
        }
        if let Some(entry) = self.entries.iter().find(|entry| {
            sc::classify_repetition(&entry.request, &request) != sc::Repetition::Distinct
        }) {
            if sc::classify_repetition(&entry.request, &request) == sc::Repetition::SameRequest {
                return entry.lines[1..].to_vec();
            }
            let refusal = self.refusal(&request, sc::RefusalReason::KeyConflict);
            return self.answer(&request, vec![refusal], false, store).0;
        }
        if !self
            .authority()
            .succeeds_within_incarnation(&request.addressed)
        {
            // Another incarnation, or this very owner: not answerable here.
            let mut refusal = self.refusal(&request, sc::RefusalReason::NotPermitted);
            if let sc::Record::Refusal(inner) = &mut refusal {
                inner.detail = Some(sc::DisclosedText::Present {
                    text: "addressed incarnation is not the one this owner holds".to_owned(),
                });
            }
            return self.answer(&request, vec![refusal], true, store).0;
        }
        let answer = match found {
            Found::Attached => Ok((sc::State::Unattached, sc::State::Attached)),
            Found::Absent => Err(sc::RefusalReason::RootAbsent),
            Found::Unattached => Err(sc::RefusalReason::TransitionFailed),
        };
        self.admit_and_answer(request, answer, None, store).0
    }

    /// An admission-stage refusal of `request` from this owner. Its
    /// responder is named only where the contract lets this owner answer.
    fn refusal(&self, request: &sc::Request, reason: sc::RefusalReason) -> sc::Record {
        let me = self.authority();
        let responder = match reason {
            sc::RefusalReason::StaleAuthority => Some(me),
            sc::RefusalReason::OwnerLive => None,
            _ if request.operation == sc::Operation::Recover => me
                .succeeds_within_incarnation(&request.addressed)
                .then_some(me),
            _ => (request.addressed == me).then_some(me),
        };
        sc::Record::Refusal(sc::Refusal {
            protocol: sc::PROTOCOL.to_owned(),
            request_key: request.request_key.clone(),
            requester: request.requester.clone(),
            addressed: request.addressed.clone(),
            operation: request.operation,
            stage: reason.stage(),
            reason,
            responder,
            detail: None,
            observed_at_unix_ms: now_ms(),
        })
    }

    /// Reports admission-stage refusals; `keep` commits them (with the
    /// request and a refused outcome) so a repeat is replayed unchanged.
    fn answer(
        &mut self,
        request: &sc::Request,
        records: Vec<sc::Record>,
        keep: bool,
        store: &mut Store,
    ) -> (Vec<String>, Option<Effect>) {
        let mut records = records;
        if keep {
            records.push(sc::Record::Outcome(sc::Outcome {
                protocol: sc::PROTOCOL.to_owned(),
                request_key: request.request_key.clone(),
                requester: request.requester.clone(),
                addressed: request.addressed.clone(),
                result: sc::OutcomeResult::Refused,
                uncertainty: None,
                reporter: None,
                observed_at_unix_ms: now_ms(),
            }));
        }
        let lines: Vec<String> = records.iter().map(line).collect();
        if !keep {
            return (lines, None);
        }
        let Ok(mut trace) = sc::RequestTrace::new(request.clone()) else {
            return (lines, None);
        };
        if records.iter().any(|record| trace.accept(record).is_err()) {
            return (lines, None);
        }
        let request_line = line(&sc::Record::Request(request.clone()));
        let mut rows = vec![self.row(request, "request", &request_line)];
        rows.extend(lines.iter().zip(&records).map(|(text, record)| {
            let kind = match record {
                sc::Record::Refusal(_) => "refusal",
                _ => "outcome",
            };
            self.row(request, kind, text)
        }));
        if store.append_control(&rows).is_ok() {
            let mut kept = vec![request_line];
            kept.extend(lines.iter().cloned());
            self.entries.push(Entry {
                request: request.clone(),
                trace,
                lines: kept,
            });
        }
        (lines, None)
    }

    fn unknown(&self, request: &sc::Request) -> sc::Record {
        sc::Record::Outcome(sc::Outcome {
            protocol: sc::PROTOCOL.to_owned(),
            request_key: request.request_key.clone(),
            requester: request.requester.clone(),
            addressed: request.addressed.clone(),
            result: sc::OutcomeResult::Unknown,
            uncertainty: Some(sc::Uncertainty::EvidenceUnavailable),
            reporter: Some(self.authority()),
            observed_at_unix_ms: now_ms(),
        })
    }

    /// The root's settlement as observations from this owner, their
    /// warranted reading, and whether retirement is described as eligible.
    pub(crate) fn settlement(
        &self,
        facts: &SettlementFacts,
        mut blocking: Vec<String>,
    ) -> (Vec<String>, Value) {
        let me = self.authority();
        let lineage = sc::Lineage {
            root: self.root.clone(),
            authorities: vec![me.clone()],
        };
        let at = now_ms();
        let observe = |subject: &sc::LogicalRef, fact: sc::Fact| sc::Observation {
            protocol: sc::PROTOCOL.to_owned(),
            reporter: me.clone(),
            subject: subject.clone(),
            fact,
            evidence: Vec::new(),
            observed_at_unix_ms: at,
        };
        let not_applicable = Some(sc::MissingReason::NotApplicable);
        let mut observations = Vec::new();
        let mut subjects = Vec::new();
        for input in &facts.inputs {
            let subject = sc::LogicalRef {
                root: self.root.clone(),
                child: None,
                work: Some(input.harness.clone()),
                input: Some(input.index.to_string()),
            };
            let insertion = if input.acknowledged {
                sc::InsertionState::Acknowledged
            } else if input.not_inserted {
                sc::InsertionState::NotInserted
            } else {
                sc::InsertionState::Uncertain
            };
            let (tagged, tagged_reason) = match (input.acknowledged, input.turn_ended) {
                (true, true) => (sc::TaggedEndState::Observed, None),
                (true, false) => (sc::TaggedEndState::Absent, None),
                (false, _) => (sc::TaggedEndState::Missing, not_applicable),
            };
            let (debt, debt_reason) = if input.acknowledged && input.turn_ended {
                (sc::LogicalSettlementState::Settled, None)
            } else if input.not_inserted {
                (sc::LogicalSettlementState::Missing, not_applicable)
            } else {
                (sc::LogicalSettlementState::Owed, None)
            };
            let mine = [
                observe(
                    &subject,
                    sc::Fact::Insertion {
                        state: insertion,
                        missing_reason: None,
                    },
                ),
                observe(
                    &subject,
                    sc::Fact::TaggedEnd {
                        state: tagged,
                        missing_reason: tagged_reason,
                    },
                ),
                observe(
                    &subject,
                    sc::Fact::LogicalSettlement {
                        state: debt,
                        missing_reason: debt_reason,
                    },
                ),
            ];
            let reading = sc::read_settlement(&subject, Some(&lineage), &mine);
            if !matches!(
                reading.logical,
                sc::LogicalReading::Settled | sc::LogicalReading::NotInserted
            ) || reading.basis != sc::Basis::Warranted
            {
                blocking.push(format!(
                    "input {}:{} reads {}",
                    input.harness,
                    input.index,
                    label(&reading.logical)
                ));
            }
            subjects.push(json!({ "subject": subject, "reading": reading }));
            observations.extend(mine);
        }
        let root_subject = sc::LogicalRef {
            root: self.root.clone(),
            child: None,
            work: None,
            input: None,
        };
        let root_debt = if facts.async_owed > 0 {
            blocking.push(format!("{} async completion(s) owed", facts.async_owed));
            sc::LogicalSettlementState::Owed
        } else {
            sc::LogicalSettlementState::Settled
        };
        let physical = if facts.works_open > 0 || facts.incarnations_open > 0 {
            sc::PhysicalCustodyState::Live
        } else if facts.works_unknown > 0 || facts.incarnations_unknown > 0 {
            sc::PhysicalCustodyState::Unsettled
        } else if facts.incarnations_observed > 0 {
            sc::PhysicalCustodyState::ExitedWaitPending
        } else if facts.incarnations == 0 {
            sc::PhysicalCustodyState::Missing
        } else {
            sc::PhysicalCustodyState::ExitedWaited
        };
        if !matches!(
            physical,
            sc::PhysicalCustodyState::ExitedWaited | sc::PhysicalCustodyState::Missing
        ) {
            blocking.push(format!(
                "root physical custody {}",
                physical_label(physical)
            ));
        }
        let root_facts = [
            observe(
                &root_subject,
                sc::Fact::LogicalSettlement {
                    state: root_debt,
                    missing_reason: None,
                },
            ),
            observe(
                &root_subject,
                sc::Fact::PhysicalCustody {
                    state: physical,
                    missing_reason: (physical == sc::PhysicalCustodyState::Missing)
                        .then_some(sc::MissingReason::NotApplicable),
                },
            ),
        ];
        let root_reading = sc::read_settlement(&root_subject, Some(&lineage), &root_facts);
        subjects.push(json!({ "subject": root_subject, "reading": root_reading }));
        observations.extend(root_facts);
        let pending = self.pending().count();
        if pending > 0 {
            blocking.push(format!("{pending} control intent(s) pending"));
        }
        if self.unreadable > 0 {
            blocking.push(format!(
                "{} kept control record(s) unreadable",
                self.unreadable
            ));
        }
        let lines = observations
            .into_iter()
            .map(|observation| line(&sc::Record::Observation(observation)))
            .collect();
        let summary = json!({
            "protocol": sc::PROTOCOL,
            "lineage": lineage,
            "lineage_meaning": "this owner alone, reading facts every owner generation committed under the store's generation fence",
            "subjects": subjects,
            "retirement": {
                "eligible": blocking.is_empty(),
                "blocking": blocking,
                "meaning": "eligible only when every input reads settled or not_inserted with warranted basis, no async completion is owed, every launch and incarnation is recorded ended by its actual waiter, and no control intent is pending; the decision to discard is the caller's",
            },
        });
        (lines, summary)
    }
}

fn label(reading: &sc::LogicalReading) -> String {
    serde_json::to_value(reading)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn physical_label(state: sc::PhysicalCustodyState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The control-only diagnostic: it disables control through this
/// vocabulary for that line and nothing else.
pub(crate) fn unavailable(diagnostic: &sc::ControlUnavailable) -> String {
    json!({ "event": "session-control-unavailable", "diagnostic": diagnostic }).to_string()
}

/// A live owner's answer to a recover request it cannot be asked: the
/// claim itself was refused because an owner holds the root. Not kept.
pub(crate) fn refuse_owner_live(text: &str) -> Option<String> {
    let Ok(sc::Record::Request(request)) = sc::Record::decode_line(text) else {
        return None;
    };
    if request.operation != sc::Operation::Recover {
        return None;
    }
    let refusal = sc::Record::Refusal(sc::Refusal {
        protocol: sc::PROTOCOL.to_owned(),
        request_key: request.request_key.clone(),
        requester: request.requester.clone(),
        addressed: request.addressed.clone(),
        operation: request.operation,
        stage: sc::RefusalStage::Admission,
        reason: sc::RefusalReason::OwnerLive,
        responder: None,
        detail: None,
        observed_at_unix_ms: now_ms(),
    });
    Some(line(&refusal))
}

/// Discovery's descriptive address of the root stored at `dir`, for
/// `requester`, from `describer` (an opaque locator, never an authority):
/// one `root_entry` record line, its authority the store's last record.
/// It claims no ownership, capacity or currency.
pub fn describe_entry(
    dir: &std::path::Path,
    requester: &str,
    describer: &str,
) -> Result<String, String> {
    let described = crate::store::describe(dir)?;
    let record = sc::Record::RootEntry(sc::RootEntry {
        protocol: sc::PROTOCOL.to_owned(),
        describer: describer.to_owned(),
        requester: requester.to_owned(),
        authority: sc::Authority {
            root: described.root_id,
            owner: described.owner_token,
            generation: described.generation.to_string(),
            incarnation: described
                .incarnation
                .map_or_else(|| "none".to_owned(), |id| id.to_string()),
        },
        observed_at_unix_ms: now_ms(),
    });
    let text = record.encode_line();
    sc::Record::decode_line(&text).map_err(|error| error.to_string())?;
    Ok(text)
}

/// Whether a control line is a v2 record rather than a stdin command.
pub(crate) fn is_record(value: &Value) -> bool {
    value.get("kind").is_some() && value.get("cmd").is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HarnessSpec, Intent};

    struct Dir(std::path::PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str) -> Dir {
        let path = std::env::temp_dir().join(format!(
            "root-control-{name}-{}-{}",
            std::process::id(),
            crate::sys::random_hex().unwrap()
        ));
        eprintln!("owned-fixture: {}", path.display());
        Dir(path)
    }

    fn intent() -> Intent {
        Intent {
            outage_closure_cap: 1,
            delivery_attempt_cap: 1,
            cwd: "/".into(),
            harnesses: vec![HarnessSpec {
                id: "h".into(),
                argv: vec!["peer".into()],
                endpoint: crate::Endpoint::Stdio,
                session: None,
                messages: vec!["one".into()],
            }],
            workload: crate::Workload::UnprivilegedUserns {},
            children: None,
        }
    }

    fn face(store: &Store) -> Face {
        Face::load(store, "root-1", "uid:7", Some(1), Arc::default()).unwrap()
    }

    fn request(face: &Face, key: &str, operation: sc::Operation) -> sc::Request {
        sc::Request {
            protocol: sc::PROTOCOL.to_owned(),
            request_key: key.to_owned(),
            requester: "uid:7".to_owned(),
            addressed: face.authority(),
            operation,
            scope: sc::ControlScope {
                root: "root-1".to_owned(),
                child: None,
            },
            reason: None,
        }
    }

    /// A request an earlier owner admitted and never answered (it died
    /// between admission and acknowledgment) stays pending after owner
    /// churn; the successor reports it `unknown` (`authority_changed`)
    /// as itself, cannot acknowledge it, and applies nothing from it.
    /// An acknowledged hold, by contrast, holds for the successor.
    #[test]
    fn admitted_unanswered_intent_stays_pending_and_acknowledged_hold_holds() {
        let dir = dir("pending");
        let mut store = Store::claim(&dir.0, Some(&intent())).unwrap().store;
        let mut first = face(&store);
        let (lines, effect) = first.handle(
            request(&first, "k-hold", sc::Operation::InputHold),
            &mut store,
        );
        assert_eq!(effect, Some(Effect::Hold));
        assert_eq!(lines.len(), 4);
        // Stage only request, receipt and admission of a close.
        let close = request(&first, "k-close", sc::Operation::Close);
        let admitted = [
            sc::Record::Request(close.clone()),
            sc::Record::Receipt(sc::Receipt {
                protocol: sc::PROTOCOL.to_owned(),
                request_key: close.request_key.clone(),
                requester: close.requester.clone(),
                addressed: close.addressed.clone(),
                durable: true,
                observed_at_unix_ms: 1,
            }),
            sc::Record::Admission(sc::Admission {
                protocol: sc::PROTOCOL.to_owned(),
                request_key: close.request_key.clone(),
                requester: close.requester.clone(),
                addressed: close.addressed.clone(),
                operation: sc::Operation::Close,
                responder: first.authority(),
                observed_at_unix_ms: 2,
            }),
        ];
        let rows: Vec<ControlRow> = admitted
            .iter()
            .map(|record| first.row(&close, "staged", &record.encode_line()))
            .collect();
        store.append_control(&rows).unwrap();
        drop(store);

        let mut store = Store::claim(&dir.0, None).unwrap().store;
        let hold = Arc::new(InputHold::default());
        let mut second = Face::load(&store, "root-1", "uid:7", Some(1), Arc::clone(&hold)).unwrap();
        assert!(hold.held(), "an acknowledged hold holds for the successor");
        assert_eq!(
            second.lifecycle_state(),
            sc::State::Open,
            "an unanswered close is not applied"
        );
        let state = second.state();
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].status, sc::PendingStatus::Admitted);
        assert_eq!(state.pending[0].request.request_key, "k-close");
        let reported = second.report_inherited(&mut store);
        assert_eq!(reported.len(), 1);
        let sc::Record::Outcome(outcome) = sc::Record::decode_line(&reported[0]).unwrap() else {
            panic!("not an outcome");
        };
        assert_eq!(outcome.result, sc::OutcomeResult::Unknown);
        assert_eq!(outcome.uncertainty, Some(sc::Uncertainty::AuthorityChanged));
        assert_eq!(outcome.reporter, Some(second.authority()));
        // The same close re-sent is the same request: replayed, not applied.
        let (replayed, effect) = second.handle(close.clone(), &mut store);
        assert_eq!(effect, None);
        assert_eq!(replayed.len(), 3);
        // Settlement cannot be retirement while it is pending.
        let facts = store.settlement_facts().unwrap();
        let (_, summary) = second.settlement(&facts, Vec::new());
        assert_eq!(summary["retirement"]["eligible"], false, "{summary}");
        drop(store);

        // A third owner still sees the earlier knowledge, unchanged.
        let store = Store::claim(&dir.0, None).unwrap().store;
        let third = face(&store);
        let entry = third
            .entries
            .iter()
            .find(|entry| entry.request.request_key == "k-close")
            .unwrap();
        assert_eq!(entry.trace.outcomes().len(), 1);
        assert!(entry.trace.acknowledgment().is_none());
    }

    /// The describer reads a root's authority without claiming or locking
    /// its store, even while an owner holds it; a store of another version
    /// is refused, not read.
    #[test]
    fn describe_reads_the_authority_without_claiming() {
        let dir = dir("describe");
        let store = Store::claim(&dir.0, Some(&intent())).unwrap().store;
        let described = crate::store::describe(&dir.0).unwrap();
        assert_eq!(described.generation, store.generation());
        assert_eq!(described.owner_token, store.owner_token());
        assert_eq!(described.incarnation, None);
        let entry = describe_entry(&dir.0, "uid:7", "frontdoor").unwrap();
        let sc::Record::RootEntry(entry) = sc::Record::decode_line(&entry).unwrap() else {
            panic!("not a root entry");
        };
        assert_eq!(entry.authority.generation, store.generation().to_string());
        assert_eq!(entry.authority.incarnation, "none");
        assert_eq!(entry.describer, "frontdoor");
        drop(store);
        let next = Store::claim(&dir.0, None).unwrap().store;
        assert_eq!(
            next.generation(),
            described.generation + 1,
            "describing claimed nothing"
        );
        drop(next);
        rusqlite::Connection::open(dir.0.join(crate::store::DB_FILE))
            .unwrap()
            .execute_batch("PRAGMA user_version = 11;")
            .unwrap();
        assert!(
            crate::store::describe(&dir.0)
                .unwrap_err()
                .contains("version")
        );
    }
}
