use oulipoly_state::deployment::row_version::checksum::{
    payload_hash_for_columns, payload_hash_for_row,
};
use oulipoly_state::deployment::row_version::compare::{
    ApplyDecision, RowSnapshot, RowVersion, decide_apply, same_or_higher,
};
use oulipoly_state::deployment::row_version::registry::{
    REGISTRY, RowKind, TableRegistration, lookup,
};
use rusqlite::Connection;
use rusqlite::types::Value;

#[test]
fn ti_18_equal_version_with_different_payload_hash_is_a_hard_conflict() {
    let source = snapshot(5, [0u8; 32]);
    let target = snapshot(5, [1u8; 32]);

    assert_eq!(
        decide_apply(&source, &target),
        ApplyDecision::ConflictEqualVersionMismatch
    );
}

#[test]
fn decide_apply_matches_the_contract_truth_table() {
    let equal_payload = [7u8; 32];
    let different_payload = [9u8; 32];

    assert_eq!(
        decide_apply(&snapshot(6, different_payload), &snapshot(5, equal_payload)),
        ApplyDecision::ApplySource
    );
    assert_eq!(
        decide_apply(&snapshot(6, equal_payload), &snapshot(5, equal_payload)),
        ApplyDecision::ApplySource
    );
    assert_eq!(
        decide_apply(&snapshot(4, different_payload), &snapshot(5, equal_payload)),
        ApplyDecision::SkipSourceSameOrHigher
    );
    assert_eq!(
        decide_apply(&snapshot(4, equal_payload), &snapshot(5, equal_payload)),
        ApplyDecision::SkipSourceSameOrHigher
    );
    assert_eq!(
        decide_apply(&snapshot(5, equal_payload), &snapshot(5, equal_payload)),
        ApplyDecision::SkipSourceSameOrHigher
    );
    assert_eq!(
        decide_apply(&snapshot(5, different_payload), &snapshot(5, equal_payload)),
        ApplyDecision::ConflictEqualVersionMismatch
    );
}

#[test]
fn registry_payload_columns_drive_compare_decisions_for_a_real_table() {
    for entry in REGISTRY {
        assert!(!entry.payload_columns.is_empty(), "{}", entry.table);
    }

    let reg = lookup("invocations").expect("registry must contain invocations");

    let payload_a = reg
        .payload_columns
        .iter()
        .enumerate()
        .map(|(idx, _col)| Some(Value::Text(format!("col-{idx}-A"))))
        .collect::<Vec<_>>();
    let payload_b = reg
        .payload_columns
        .iter()
        .enumerate()
        .map(|(idx, _col)| Some(Value::Text(format!("col-{idx}-B"))))
        .collect::<Vec<_>>();

    let hash_a = payload_hash_for_columns(&payload_a);
    let hash_b = payload_hash_for_columns(&payload_b);
    assert_ne!(
        hash_a, hash_b,
        "different registry-driven payloads must hash differently"
    );

    let source = snapshot(7, hash_a);
    let target = snapshot(7, hash_a);
    assert_eq!(
        decide_apply(&source, &target),
        ApplyDecision::SkipSourceSameOrHigher
    );

    let source = snapshot(7, hash_a);
    let target = snapshot(7, hash_b);
    assert_eq!(
        decide_apply(&source, &target),
        ApplyDecision::ConflictEqualVersionMismatch
    );
}

#[test]
fn compare_consumes_real_checksum_hashes_for_equal_version_conflict() {
    let payload_match = vec![
        Some(Value::Text("alpha".into())),
        Some(Value::Integer(42)),
        Some(Value::Null),
    ];
    let payload_drift = vec![
        Some(Value::Text("alpha".into())),
        Some(Value::Integer(43)),
        Some(Value::Null),
    ];

    let h_match = payload_hash_for_columns(&payload_match);
    let h_drift = payload_hash_for_columns(&payload_drift);
    assert_ne!(h_match, h_drift);

    let source = snapshot(11, h_match);
    let target = snapshot(11, h_match);
    assert_eq!(
        decide_apply(&source, &target),
        ApplyDecision::SkipSourceSameOrHigher
    );

    let source = snapshot(11, h_match);
    let target = snapshot(11, h_drift);
    assert_eq!(
        decide_apply(&source, &target),
        ApplyDecision::ConflictEqualVersionMismatch
    );

    let source = snapshot(12, h_drift);
    let target = snapshot(11, h_match);
    assert_eq!(decide_apply(&source, &target), ApplyDecision::ApplySource);
}

#[test]
fn same_or_higher_is_true_iff_target_version_is_at_least_candidate_version() {
    assert!(same_or_higher(RowVersion(5), RowVersion(5)));
    assert!(!same_or_higher(RowVersion(5), RowVersion(6)));
    assert!(same_or_higher(RowVersion(6), RowVersion(5)));
}

#[test]
fn payload_hash_for_columns_is_deterministic() {
    let values = vec![
        Some(Value::Text("fixture".to_string())),
        Some(Value::Integer(42)),
        Some(Value::Real(1.25)),
        Some(Value::Null),
    ];

    assert_eq!(
        payload_hash_for_columns(&values),
        payload_hash_for_columns(&values)
    );
}

#[test]
fn payload_hash_for_columns_distinguishes_null_from_empty_string() {
    let null_hash = payload_hash_for_columns(&[Some(Value::Null)]);
    let empty_string_hash = payload_hash_for_columns(&[Some(Value::Text(String::new()))]);

    assert_ne!(null_hash, empty_string_hash);
}

#[test]
fn payload_hash_for_row_preserves_sqlite_null_distinct_from_absent_value() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE sample (
            id INTEGER PRIMARY KEY,
            payload TEXT NULL,
            row_version INTEGER NOT NULL DEFAULT 0
        );
        INSERT INTO sample (id, payload) VALUES (1, NULL);",
    )
    .unwrap();
    let registration = TableRegistration {
        table: "sample",
        primary_key_columns: &["id"],
        payload_columns: &["payload"],
        kind: RowKind::Mutable,
    };

    let row_hash = conn
        .query_row("SELECT payload FROM sample WHERE id = 1", [], |row| {
            payload_hash_for_row(&registration, row)
        })
        .unwrap();

    assert_eq!(row_hash, payload_hash_for_columns(&[Some(Value::Null)]));
    assert_ne!(row_hash, payload_hash_for_columns(&[None]));
}

fn snapshot(version: i64, payload_hash: [u8; 32]) -> RowSnapshot {
    RowSnapshot {
        version: RowVersion(version),
        payload_hash,
    }
}
