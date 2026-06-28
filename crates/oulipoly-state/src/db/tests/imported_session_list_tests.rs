use super::common::*;
use super::*;

#[test]
fn imported_session_list_includes_imported_and_owned_rows_with_stable_ordering() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a",
        "provider-a-native",
        "<unknown>",
        "2026-06-01T00:01:00Z",
    );
    seed_imported_metadata(
        &db,
        "provider-a",
        "provider-a-native",
        Some("Provider A imported"),
        Some("/tmp/provider-a"),
        "2026-06-01T00:05:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "provider-b",
        "provider-b-native",
        "<unknown>",
        "2026-06-01T00:05:00Z",
    );
    seed_imported_metadata(
        &db,
        "provider-b",
        "provider-b-native",
        None,
        None,
        "2026-06-01T00:05:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_C,
        "opencode",
        "owned-native",
        "opencode-model",
        "2026-06-01T00:04:00Z",
    );
    db.ingest_session_turns_batch(
        "opencode",
        &[
            turn("owned-native", "owned-turn-1", "2026-06-01T00:04:01Z"),
            turn("owned-native", "owned-turn-2", "2026-06-01T00:04:02Z"),
        ],
    )
    .unwrap();

    let rows = db.imported_session_list().unwrap();

    assert_eq!(
        rows.iter()
            .map(|row| (
                row.active_provider.as_str(),
                row.active_provider_session_id.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("provider-a", "provider-a-native"),
            ("provider-b", "provider-b-native"),
            ("opencode", "owned-native"),
        ]
    );
    assert_eq!(rows[0].chain_id, CHAIN_A);
    assert_eq!(rows[0].title.as_deref(), Some("Provider A imported"));
    assert_eq!(rows[0].cwd.as_deref(), Some("/tmp/provider-a"));
    assert_eq!(rows[0].last_used_or_updated_at, ts("2026-06-01T00:05:00Z"));
    assert_eq!(rows[0].turn_count, 0);
    assert!(rows[0].is_imported);
    assert_eq!(rows[2].turn_count, 2);
    assert!(!rows[2].is_imported);
}

fn seed_imported_metadata(
    db: &StateDb,
    provider_name: &str,
    provider_session_id: &str,
    title: Option<&str>,
    cwd: Option<&str>,
    provider_updated_at: &str,
) {
    db.upsert_imported_session_display_metadata(&ImportedSessionDisplayMetadataUpsert {
        provider_name: provider_name.to_string(),
        provider_session_id: provider_session_id.to_string(),
        title: title.map(str::to_string),
        cwd: cwd.map(str::to_string),
        turn_count: Some(99),
        provider_updated_at: Some(ts(provider_updated_at)),
        seen_at: ts("2026-06-01T00:06:00Z"),
    })
    .unwrap();
}

fn turn(session_id: &str, turn_id: &str, timestamp: &str) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        timestamp: ts(timestamp),
        role: "user".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: None,
    }
}
