//! Read-only SDK adapter used by the packaged caller. No ownership or effects.
use agent_provider_contract::session_control as sc;
use serde_json::{Value, json};

fn decode(value: &Value) -> Result<sc::Record, sc::ControlUnavailable> {
    sc::Record::decode_line(&value.to_string())
}

/// Select actual advertised capabilities, then check both submission and
/// immutable original trace joins. Only accepted records and generic rejection
/// diagnostics are returned; rejected input can be a private handle by mistake.
pub fn read(value: &Value) -> Value {
    match read_selected(value) {
        Ok(reading) => reading,
        Err(diagnostic) => json!({"class": "control-unavailable", "diagnostics": [diagnostic]}),
    }
}

fn read_selected(value: &Value) -> Result<Value, sc::ControlUnavailable> {
    let selected = sc::select(&crate::control::offer(), &value["advertisement"])?;
    let sc::Record::ControlState(state) = decode(&value["state"])? else {
        return Err(sc::ControlUnavailable::new(
            sc::UnavailableReason::InvalidRecord,
            "expected inspection",
        ));
    };
    state.agree(&selected)?;
    if value["request"].is_null() {
        return Ok(json!({"class": "inspected", "selected": selected}));
    }
    let sc::Record::Request(submitted) = decode(&value["request"])? else {
        return Err(sc::ControlUnavailable::new(
            sc::UnavailableReason::InvalidRecord,
            "expected submission",
        ));
    };
    submitted.agree(&selected)?;
    if submitted.addressed.root != state.reporter.root {
        return Err(sc::ControlUnavailable::new(
            sc::UnavailableReason::ProtocolViolation,
            "inspection names another root",
        ));
    }
    let original = if value["original"].is_null() {
        submitted.clone()
    } else if let sc::Record::Request(original) = decode(&value["original"])? {
        original
    } else {
        return Err(sc::ControlUnavailable::new(
            sc::UnavailableReason::InvalidRecord,
            "expected original",
        ));
    };
    original.agree(&selected)?;
    let mut trace = sc::RequestTrace::new(original.clone())?;
    let mut steps = Vec::new();
    let mut rejected = Vec::new();
    let mut prior_claims = Vec::new();
    let mut claims = Vec::new();
    let mut conflict = false;
    for record in value["prior_claims"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|record| (record, true))
        .chain(
            value["claims"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|record| (record, false)),
        )
    {
        let (record, is_prior) = record;
        let mut accepted = None;
        let result = decode(record).and_then(|claim| {
            if let sc::Record::Conflict(answer) = &claim {
                answer.answer_to(&submitted)?;
            } else if submitted != original
                && sc::classify_repetition(&original, &submitted) != sc::Repetition::KeyConflict
            {
                return Err(sc::ControlUnavailable::new(
                    sc::UnavailableReason::ProtocolViolation,
                    "submission is unrelated to original",
                ));
            }
            let step = trace.accept(&claim)?;
            accepted = Some(claim);
            Ok(step)
        });
        match result {
            Ok(step) => {
                conflict |= matches!(step, sc::Step::SubmissionConflict { .. });
                steps.push(step);
                if is_prior {
                    prior_claims.push(accepted.unwrap());
                } else {
                    claims.push(accepted.unwrap());
                }
            }
            Err(diagnostic) => rejected.push(json!({"diagnostic": diagnostic})),
        }
    }
    let class = if !rejected.is_empty() {
        "incomplete"
    } else if conflict {
        "control-refused"
    } else if submitted != original {
        "incomplete"
    } else {
        match trace.outcome().map(|outcome| outcome.result) {
            Some(sc::OutcomeResult::Acknowledged) => "acknowledged",
            Some(sc::OutcomeResult::Fulfilled) => "fulfilled",
            Some(sc::OutcomeResult::Unfulfilled) => "unfulfilled",
            Some(sc::OutcomeResult::Refused) => "control-refused",
            Some(sc::OutcomeResult::Unknown) => "control-unknown",
            None => "incomplete",
        }
    };
    Ok(
        json!({"class": class, "selected": selected, "request": sc::Record::Request(submitted),
        "original": sc::Record::Request(original), "claims": claims, "prior_claims": prior_claims,
        "steps": steps, "rejected": rejected, "outcome": trace.outcome(),
        "admission": trace.admission(), "acknowledgment": trace.acknowledgment(),
        "fulfillment": trace.fulfillment(), "non_fulfillment": trace.non_fulfillment(),
        "current_relation": trace.relate(&state)}),
    )
}
