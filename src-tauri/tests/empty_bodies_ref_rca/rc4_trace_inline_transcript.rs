use super::{INVOCATION_UUID, MODEL, PROVIDER, RcaFixture, SESSION_ID};
use agent_runner_lib::state::InvocationStart;
use agent_runner_lib::trace::{TraceOptions, trace_invocation};

/// RC-4 — `trace --json --inline-transcript` is a null placeholder and does
/// not inline DB-stored turn bodies.
///
/// Design-intent source: user report for Phase 0 says routing/export/resume
/// consumers should be able to use turn bodies from `state.db`; the current
/// trace contract exposes `transcript: null`, demonstrating the missing
/// DB-backed body read path.
#[test]
fn trace_inline_transcript_embeds_db_stored_turn_bodies() {
    let fixture = RcaFixture::new();
    let db = fixture.open_db();
    fixture.add_contract_body_column();
    fixture.seed_body_turns();
    let invocation_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: INVOCATION_UUID.to_string(),
            model_name: MODEL.to_string(),
            provider_name: PROVIDER.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(invocation_id, Some(SESSION_ID), "fixture")
        .unwrap();
    db.finalize_invocation(invocation_id, true, 0, None, None)
        .unwrap();

    let report = trace_invocation(
        &db,
        INVOCATION_UUID,
        TraceOptions {
            max_depth: 64,
            json: true,
            inline_transcript: true,
            transcript: false,
        },
    )
    .unwrap();
    let json = serde_json::to_value(&report).unwrap();

    assert!(
        json["root"]["transcript"].is_array(),
        "inline transcript must embed DB-stored turn bodies; got {}",
        json["root"]["transcript"]
    );
    assert!(
        json["root"]["transcript"]
            .to_string()
            .contains("db stored assistant body"),
        "inline transcript must include stored assistant content: {}",
        json["root"]["transcript"]
    );
}
