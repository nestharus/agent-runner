mod common;

use oulipoly_agent_messenger::{ReturnRequest, return_artifact};
use uuid::Uuid;

// proposal § Test-Intent Track row: verdict override/carry-through
// contract § Expected observable signals row: verdict override
// named risk: Messenger Domain Layer HIGH - verdict_line could diverge between source metadata, store metadata, and returned receipt
// selected level: library_integration
#[test]
fn verdict_override_wins_over_scratchpad_source_verdict() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    common::write_scratchpad(
        &db,
        invocation,
        "draft.md",
        common::text_bytes(),
        Some("text/markdown"),
        Some("SOURCE: old"),
    );

    let receipt = return_artifact(ReturnRequest {
        verdict_line: Some("OVERRIDE: final".to_string()),
        ..common::scratchpad_request(&db, invocation, "proposal.md", "draft.md")
    })
    .expect("return with override");

    assert_eq!(receipt.verdict_line.as_deref(), Some("OVERRIDE: final"));
}

// proposal § Test-Intent Track row: verdict carry-through
// contract § Operation semantics scratchpad source metadata
// named risk: Messenger Domain Layer HIGH - scratchpad source verdict could be dropped when returning without override
// selected level: library_integration
#[test]
fn absent_override_carries_scratchpad_source_verdict() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    common::write_scratchpad(
        &db,
        invocation,
        "draft.md",
        common::text_bytes(),
        Some("text/markdown"),
        Some("SOURCE: usable"),
    );

    let receipt = return_artifact(common::scratchpad_request(
        &db,
        invocation,
        "proposal.md",
        "draft.md",
    ))
    .expect("return without override");

    assert_eq!(receipt.verdict_line.as_deref(), Some("SOURCE: usable"));
    assert_eq!(receipt.format_hint.as_deref(), Some("text/markdown"));
}

// proposal § Test-Intent Track row: inline absent verdict remains null
// named risk: Messenger Domain Layer HIGH - inline returns could inherit stale verdict metadata from unrelated state
// selected level: library_integration
#[test]
fn inline_return_without_verdict_has_null_verdict() {
    let (db, _store) = common::init_temp_store();
    let invocation = Uuid::new_v4();

    let receipt = return_artifact(common::inline_request(
        &db,
        invocation,
        "inline.md",
        common::text_bytes(),
    ))
    .expect("inline return");

    assert_eq!(receipt.verdict_line, None);
    assert_eq!(receipt.format_hint, None);
}
