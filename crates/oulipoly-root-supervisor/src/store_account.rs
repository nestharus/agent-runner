//! The account of a root store read without claiming it: what each logical
//! input became as its owners committed it, for a requester after its owner,
//! entry or front door was lost.
//!
//! The reader holds no owner generation, takes no lock, writes nothing,
//! starts nothing and authorizes no replay. Per-input facts use the owner's
//! own rules ([`crate::control::input_facts`]); the reading's basis is
//! `unwarranted` (records of the store's owners, read by a non-owner), so it
//! is never retirement evidence. An attempt with no recorded outcome stays
//! unknown: its owner may or may not have sent it. Metadata only: no message
//! text, control lines, argv, cwd, environment, Bash commands or output.
use std::collections::BTreeMap;
use std::path::Path;

use agent_provider_contract::session_control as sc;
use serde_json::{Value, json};

use crate::store::{AttemptOutcome, InputRecord, UNKNOWN_PRIOR_OWNER};

/// Runner schema of this account. Consumers select on it, not on a binary.
pub const SCHEMA: &str = "oulipoly.root_store_account/v1";
/// Inputs listed in one account; the rest are counted, not listed.
pub const MAX_INPUTS: usize = 256;
/// Distinct attempt outcome labels listed per input.
const MAX_LABELS: usize = 8;

/// One `root_store_account` line for the store at `dir`, or why not.
pub fn account(dir: &Path, requester: &str) -> Result<String, String> {
    let mut reading = crate::store::read_unclaimed(dir, MAX_INPUTS)?;
    let required = crate::account::Account::load(reading.required_account.take());
    reading.facts.required_account_incomplete = required.physical_incomplete();
    let described = &reading.described;
    // The store's last recorded authority; never emitted as a reporter.
    let recorded = sc::Authority {
        root: described.root_id.clone(),
        owner: described.owner_token.clone(),
        generation: described.generation.to_string(),
        incarnation: described
            .incarnation
            .map_or_else(|| "none".to_owned(), |id| id.to_string()),
    };
    let mut readings: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_input = BTreeMap::new();
    for input in &reading.facts.inputs {
        let subject = crate::control::input_subject(&described.root_id, input);
        let observations = crate::control::input_facts(input).map(|fact| sc::Observation {
            protocol: sc::PROTOCOL.to_owned(),
            reporter: recorded.clone(),
            subject: subject.clone(),
            fact,
            evidence: Vec::new(),
            observed_at_unix_ms: 0,
        });
        let settlement = sc::read_settlement(&subject, None, &observations);
        *readings.entry(label(&settlement.logical)).or_default() += 1;
        by_input.insert(
            (input.harness.clone(), input.index),
            (input.clone(), settlement),
        );
    }
    let inputs: Vec<Value> = reading
        .inputs
        .iter()
        .map(|record| {
            let facts = by_input.get(&(record.harness.clone(), record.index));
            input_account(record, facts.map(|(f, _)| f), facts.map(|(_, s)| s))
        })
        .collect();
    let mut blocking = vec![
        "store read without an owner: basis unwarranted, never retirement evidence".to_owned(),
    ];
    for (logical, count) in &readings {
        if logical != "settled" && logical != "not_inserted" {
            blocking.push(format!("{count} input(s) read {logical}"));
        }
    }
    if reading.facts.async_owed > 0 {
        blocking.push(format!(
            "{} async completion(s) owed",
            reading.facts.async_owed
        ));
    }
    let physical = crate::control::recorded_custody(&reading.facts, &mut blocking);
    let listed = inputs.len();
    let omitted = usize::try_from(reading.inputs_total)
        .unwrap_or(usize::MAX)
        .saturating_sub(listed);
    let required_snapshot = required.snapshot();
    let facts = &reading.facts;
    let value = json!({
        "kind": "root_store_account",
        "schema": SCHEMA,
        "requester": requester,
        "reader": {
            "role": "unclaimed-store-reader",
            "owner_generation_claimed": false,
            "locks_taken": false,
            "provider_started": false,
        },
        "root": described.root_id,
        "recorded_authority": {
            "generation": described.generation,
            "incarnation": described.incarnation,
            "owner_generations": reading.owner_generations,
            "last_claimed_unix": reading.last_claimed_unix,
            "meaning": "the store's last owner claim; not proof that it or any owner is live or current",
        },
        "inputs": inputs,
        "inputs_total": reading.inputs_total,
        "inputs_listed": listed,
        "inputs_omitted": omitted,
        "readings": readings,
        "control": { "kept_claims": reading.control_kinds, "lines": "not-read" },
        "async_owed": facts.async_owed,
        "bash_runs": reading.bash_runs,
        "children": { "total": reading.children, "unresolved": reading.children_unresolved },
        "recorded_actor_custody": {
            "state": physical,
            "works_open": facts.works_open,
            "works_unknown": facts.works_unknown,
            "incarnations": facts.incarnations,
            "incarnations_open": facts.incarnations_open,
            "incarnations_observed": facts.incarnations_observed,
            "incarnations_unknown": facts.incarnations_unknown,
            "meaning": "store records only; an open row is an end no owner recorded: after owner loss that end is unknown, not live and not settled; not proof of actor completeness",
        },
        "required_account": {
            "counts": required_snapshot["counts"],
            "omitted": required_snapshot["omitted"],
            "prior_unavailable": required_snapshot["prior_unavailable"],
            "details_complete": required_snapshot["details_complete"],
            "persistence_failures": required_snapshot["persistence_failures"],
            "records": "not-copied",
        },
        "retirement": {
            "eligible": false,
            "blocking": blocking,
            "meaning": "an unclaimed read never establishes retirement; blocking lists what the records themselves leave open",
        },
        "complete": omitted == 0 && required_snapshot["prior_unavailable"] == false,
        "retry": "not-authorized",
        "redelivery_basis": "positive non-insertion, or proven durable same-key continuity excluding a second native effect; a key, ACK label, stop or this account alone establishes neither",
        "meaning": "per-input transport facts the store's owners committed, read after the fact: not native processing, receiver continuity, a new owner generation or final truth",
        "observed_at_unix_ms": crate::control::now_ms(),
    });
    Ok(value.to_string())
}

fn input_account(
    record: &InputRecord,
    facts: Option<&crate::store::InputFacts>,
    settlement: Option<&sc::SettlementReading>,
) -> Value {
    let mut acknowledged = 0u64;
    let mut not_inserted = 0u64;
    let mut unresolved = 0u64;
    let mut unrecorded = 0u64;
    let mut prior_owner_unknown = 0u64;
    let mut labels: BTreeMap<String, u64> = BTreeMap::new();
    let mut labels_omitted = 0u64;
    for outcome in &record.attempts {
        match AttemptOutcome::classify(outcome.as_deref()) {
            AttemptOutcome::Acknowledged => acknowledged += 1,
            AttemptOutcome::Refused | AttemptOutcome::NotSent => not_inserted += 1,
            AttemptOutcome::Unresolved => unresolved += 1,
        }
        match outcome.as_deref() {
            None => unrecorded += 1,
            Some(UNKNOWN_PRIOR_OWNER) => prior_owner_unknown += 1,
            Some(_) => {}
        }
        let name = match outcome.as_deref() {
            None => "unrecorded".to_owned(),
            Some(label) => crate::account::tag(Some(label))
                .as_str()
                .filter(|label| label.len() <= 64)
                .unwrap_or("other")
                .to_owned(),
        };
        if labels.len() < MAX_LABELS || labels.contains_key(&name) {
            *labels.entry(name).or_default() += 1;
        } else {
            labels_omitted += 1;
        }
    }
    let acked = record.ack_label.is_some();
    let turn_ended = record.turn_end_generation.is_some();
    let state = if acked && turn_ended {
        "acknowledged-turn-ended"
    } else if acked {
        "acknowledged-turn-end-unobserved"
    } else if record.stop.is_some() && facts.is_some_and(|f| f.not_inserted) {
        "stopped-not-inserted"
    } else if record.stop.is_some() {
        "stopped-insertion-unresolved"
    } else if unresolved > 0 {
        "owed-insertion-unresolved"
    } else if record.attempts.is_empty() {
        "owed-not-attempted"
    } else {
        "owed-attempts-not-inserted"
    };
    json!({
        "harness": crate::account::tag(Some(&record.harness)),
        "harness_kind": record.harness_kind,
        "index": record.index,
        "key": crate::account::tag(Some(&record.key)),
        "origin": record.origin,
        "producer": record.producer,
        "admitted_generation": record.admitted_generation,
        "state": state,
        "stop": record.stop.as_deref().map(|stop| crate::account::tag(Some(stop))),
        "ack": record.ack_label.as_ref().map(|ack| json!({
            "label": crate::account::tag(Some(ack)),
            "basis": crate::account::tag(record.ack_basis.as_deref()),
            "recovered": record.ack_recovered.unwrap_or(false),
            "generation": record.ack_generation,
            "message_id": crate::account::tag(record.ack_message_id.as_deref()),
        })),
        "tagged_end": record.turn_end_generation.map(|generation| json!({ "generation": generation })),
        "closures": record.closures,
        "completion_linked": record.completion_linked,
        "attempts": {
            "total": record.attempts.len(),
            "acknowledged": acknowledged,
            "not_inserted": not_inserted,
            "unresolved": unresolved,
            "unrecorded": unrecorded,
            "unknown_prior_owner": prior_owner_unknown,
            "labels": labels,
            "labels_omitted": labels_omitted,
            "meaning": "unrecorded: no outcome was committed by the attempt's generation; it may or may not have been sent or inserted",
        },
        "settlement": settlement,
    })
}

fn label(reading: &sc::LogicalReading) -> String {
    serde_json::to_value(reading)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DB_FILE, DurableAck, Store};
    use crate::{HarnessSpec, Intent};
    use rusqlite::Connection;

    struct Dir(std::path::PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dir(name: &str) -> Dir {
        let path = std::env::temp_dir().join(format!(
            "root-account-{name}-{}-{}",
            std::process::id(),
            crate::sys::random_hex().unwrap()
        ));
        eprintln!("owned-fixture: {}", path.display());
        Dir(path)
    }

    fn intent(messages: Vec<String>) -> Intent {
        Intent {
            outage_closure_cap: 1,
            delivery_attempt_cap: 1,
            cwd: "/fixture-private-cwd".into(),
            harnesses: vec![HarnessSpec {
                id: "h".into(),
                argv: vec!["fixture-private-argv".into()],
                endpoint: crate::Endpoint::Stdio,
                session: None,
                resident: None,
                messages,
            }],
            workload: crate::Workload::UnprivilegedUserns {},
            children: None,
        }
    }

    fn generations_and_open_attempts(dir: &Path) -> (i64, i64) {
        let conn = Connection::open(dir.join(DB_FILE)).unwrap();
        (
            conn.query_row("SELECT count(*) FROM owner", [], |r| r.get(0))
                .unwrap(),
            conn.query_row(
                "SELECT count(*) FROM attempt WHERE outcome IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap(),
        )
    }

    /// Synthetic owner loss: one input acknowledged with its tagged end, one
    /// durably stopped `rejected-unresolved` (its public event may never have
    /// been emitted), one sent with no recorded outcome, one never attempted.
    /// The unclaimed account keeps each apart and changes nothing.
    #[test]
    fn lost_owner_account_keeps_each_input_apart_without_claiming() {
        let dir = dir("loss");
        let texts: Vec<String> = (0..4)
            .map(|i| format!("fixture-private-prompt-{i}"))
            .collect();
        let mut store = Store::claim(&dir.0, Some(&intent(texts.clone())))
            .unwrap()
            .store;
        let incarnation = store
            .begin_incarnation("token", "unprivileged-userns")
            .unwrap();
        store.begin_work(0, incarnation).unwrap();
        let acked = store.begin_attempt(0, 0).unwrap();
        let ack = DurableAck {
            label: "accepted".into(),
            basis: Some("single-attempt".into()),
            recovered: false,
            generation: store.generation(),
            message_id: Some("m-0".into()),
        };
        store.record_ack(0, 0, acked, &ack).unwrap();
        store.record_turn_end(0, 0).unwrap();
        let rejected = store.begin_attempt(0, 1).unwrap();
        store
            .resolve_attempt(rejected, "rejected-unresolved")
            .unwrap();
        store.begin_attempt(0, 2).unwrap();
        // Owner loss: no outcome, no terminal, no successor.
        drop(store);
        let before = generations_and_open_attempts(&dir.0);

        let owner_token = crate::store::describe(&dir.0).unwrap().owner_token;
        let line = account(&dir.0, "uid:7").unwrap();
        assert!(!line.contains(&owner_token), "no control authority token");
        assert_eq!(generations_and_open_attempts(&dir.0), before, "read only");
        assert_eq!(before, (1, 1));
        for private in texts.iter().map(String::as_str).chain([
            "fixture-private-cwd",
            "fixture-private-argv",
            "\"token\"",
        ]) {
            assert!(!line.contains(private), "metadata only: {private}");
        }
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["schema"], SCHEMA);
        assert_eq!(value["requester"], "uid:7");
        assert_eq!(value["reader"]["owner_generation_claimed"], false);
        assert_eq!(value["recorded_authority"]["owner_generations"], 1);
        assert_eq!(value["retry"], "not-authorized");
        assert_eq!(value["retirement"]["eligible"], false);
        assert_eq!(value["inputs_omitted"], 0);
        assert_eq!(value["complete"], true);
        let inputs = value["inputs"].as_array().unwrap();
        let states: Vec<&str> = inputs
            .iter()
            .map(|i| i["state"].as_str().unwrap())
            .collect();
        assert_eq!(
            states,
            [
                "acknowledged-turn-ended",
                "stopped-insertion-unresolved",
                "owed-insertion-unresolved",
                "owed-not-attempted"
            ]
        );
        assert_eq!(inputs[0]["ack"]["label"], "accepted");
        assert_eq!(inputs[0]["tagged_end"]["generation"], 1);
        assert_eq!(inputs[0]["settlement"]["logical"], "settled");
        assert_eq!(inputs[0]["settlement"]["basis"], "unwarranted");
        assert_eq!(inputs[1]["stop"], "rejected-unresolved");
        assert_eq!(inputs[1]["attempts"]["unresolved"], 1);
        assert!(inputs[1]["ack"].is_null());
        assert_ne!(inputs[1]["settlement"]["logical"], "not_inserted");
        assert_eq!(inputs[2]["attempts"]["unrecorded"], 1);
        assert_eq!(inputs[2]["attempts"]["labels"]["unrecorded"], 1);
        assert_eq!(inputs[2]["settlement"]["insertion"]["state"], "uncertain");
        assert_eq!(value["readings"]["settled"], 1);
        assert_eq!(value["readings"]["owed"], 3);
        // The incarnation and work were never recorded ended: not settled.
        assert_eq!(value["recorded_actor_custody"]["state"], "live");
        let blocking = value["retirement"]["blocking"].to_string();
        assert!(blocking.contains("3 input(s) read owed"), "{blocking}");
        assert!(
            blocking.contains("root physical custody live"),
            "{blocking}"
        );

        // A later owner's classification stays visible as prior-owner unknown.
        drop(Store::claim(&dir.0, None).unwrap());
        let later: Value = serde_json::from_str(&account(&dir.0, "uid:7").unwrap()).unwrap();
        assert_eq!(later["inputs"][2]["attempts"]["unknown_prior_owner"], 1);
        assert_eq!(later["inputs"][2]["attempts"]["unrecorded"], 0);
        // The successor's own claim used the one-attempt budget: a durable
        // stop that still keeps the unknown attempt unresolved.
        assert_eq!(later["inputs"][2]["stop"], "attempts-exhausted");
        assert_eq!(later["inputs"][2]["state"], "stopped-insertion-unresolved");
        assert_eq!(later["inputs"][2]["attempts"]["unresolved"], 1);
        assert_eq!(later["recorded_authority"]["owner_generations"], 2);
    }

    #[test]
    fn listing_is_bounded_and_omission_is_incomplete() {
        let dir = dir("bound");
        let texts = (0..=MAX_INPUTS).map(|i| format!("m{i}")).collect();
        drop(Store::claim(&dir.0, Some(&intent(texts))).unwrap());
        let value: Value = serde_json::from_str(&account(&dir.0, "uid:7").unwrap()).unwrap();
        assert_eq!(value["inputs"].as_array().unwrap().len(), MAX_INPUTS);
        assert_eq!(value["inputs_total"], MAX_INPUTS + 1);
        assert_eq!(value["inputs_omitted"], 1);
        assert_eq!(value["complete"], false);
        assert_eq!(value["readings"]["owed"], MAX_INPUTS + 1);
    }

    #[test]
    fn other_versions_and_absent_stores_are_refused() {
        let dir = dir("version");
        assert!(account(&dir.0, "uid:7").is_err());
        drop(Store::claim(&dir.0, Some(&intent(vec!["m".into()]))).unwrap());
        Connection::open(dir.0.join(DB_FILE))
            .unwrap()
            .execute_batch("PRAGMA user_version = 99;")
            .unwrap();
        let refused = account(&dir.0, "uid:7").unwrap_err();
        assert!(refused.contains("unknown store version 99"), "{refused}");
    }
}
