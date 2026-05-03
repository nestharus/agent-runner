use super::RcaFixture;

/// RC-1 — `session_turns` has no direct body column.
///
/// Design-intent source: user report for Phase 0 states that `state.db` was
/// supposed to store every turn body directly, while the current `session_turns`
/// table stores only metadata. This contract test asserts the schema invariant
/// the fix must establish.
#[test]
fn session_turns_schema_has_direct_body_storage_column() {
    // risk: schema regression; level: particular-integration; source: contract §4 T1 / research/12-empty-bodies-ref-rca.md RC-1.
    let fixture = RcaFixture::new();
    let columns = fixture.session_turns_columns();

    assert!(
        columns.iter().any(|c| c == "body"),
        "session_turns must include a `body` TEXT column; actual columns: {columns:?}"
    );

    let conn = fixture.conn();
    let mut stmt = conn.prepare("PRAGMA table_info(session_turns)").unwrap();
    let body_columns = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let (_, body_type, body_notnull) = body_columns
        .iter()
        .find(|(name, _, _)| name == "body")
        .unwrap_or_else(|| panic!("body column row must be visible; actual: {body_columns:?}"));
    assert_eq!(body_type.to_uppercase(), "TEXT");
    assert_eq!(body_notnull, &0, "body column must be nullable");
}
