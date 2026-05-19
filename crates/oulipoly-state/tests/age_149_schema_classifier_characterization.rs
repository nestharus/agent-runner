//! ## Declared roles
//! orchestration, mapper, validator

mod fixtures;

use fixtures::v3_full_state_db::build_v3_full_state_db;
use fixtures::v3_setup_only_db::build_versionless_setup_only_db;
use fixtures::versionless_unrecognized::build_versionless_unrecognized_db;
use oulipoly_state::schema::{self, SchemaCompatibility};
use rusqlite::Connection;

#[test]
fn idx_schema_03_classify_preserves_versionless_table_shape_outcomes() {
    let empty = Connection::open_in_memory().unwrap();
    assert_eq!(
        schema::classify(&empty).unwrap(),
        SchemaCompatibility::Fresh
    );

    let dir = tempfile::tempdir().unwrap();
    let full_legacy_path = dir.path().join("full-legacy.db");
    build_v3_full_state_db(&full_legacy_path);
    let full_legacy = Connection::open(&full_legacy_path).unwrap();
    full_legacy
        .execute_batch("PRAGMA user_version = 0;")
        .unwrap();
    assert_eq!(
        schema::classify(&full_legacy).unwrap(),
        SchemaCompatibility::LegacyVersionless
    );

    let setup_only_path = dir.path().join("setup-only.db");
    build_versionless_setup_only_db(&setup_only_path);
    let setup_only = Connection::open(&setup_only_path).unwrap();
    assert_eq!(
        schema::classify(&setup_only).unwrap(),
        SchemaCompatibility::LegacyVersionless
    );

    let unrecognized_path = dir.path().join("unrecognized.db");
    build_versionless_unrecognized_db(&unrecognized_path);
    let unrecognized = Connection::open(&unrecognized_path).unwrap();
    assert_eq!(
        schema::classify(&unrecognized).unwrap(),
        SchemaCompatibility::UnrecognizedVersionless
    );
}

#[test]
fn idx_schema_04_classify_preserves_user_version_read_error_message() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("corrupt.db");
    std::fs::write(&db_path, b"not a sqlite database").unwrap();
    let conn = Connection::open(&db_path).unwrap();

    let err = schema::classify(&conn).unwrap_err();

    assert_eq!(
        err,
        "Failed to read PRAGMA user_version: file is not a database"
    );
}
