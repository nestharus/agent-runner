use super::RcaFixture;

/// RC-1 — `session_turns` has no direct body/content/payload column.
///
/// Design-intent source: user report for Phase 0 states that `state.db` was
/// supposed to store every turn body directly, while the current `session_turns`
/// table stores only metadata. This contract test asserts the schema invariant
/// the fix must establish.
#[test]
fn session_turns_schema_has_direct_body_storage_column() {
    let fixture = RcaFixture::new();
    let columns = fixture.session_turns_columns();

    assert!(
        columns
            .iter()
            .any(|column| matches!(column.as_str(), "body" | "content" | "payload")),
        "session_turns must include a direct turn body column named body/content/payload; actual columns: {columns:?}"
    );
}
