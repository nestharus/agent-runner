const EXPECTED_INVOCATIONS_SCHEMA_SNIPPET: &str = r#"CREATE TABLE IF NOT EXISTS invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_uuid TEXT NOT NULL UNIQUE,
            model_name TEXT NOT NULL,
            provider_name TEXT,
            provider_index INTEGER NOT NULL,
            parent_invocation_id INTEGER REFERENCES invocations(id),
            status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
            success INTEGER,
            exit_code INTEGER,
            error_category TEXT,
            terminal_reason TEXT,
            session_id TEXT,
            session_capture_method TEXT,
            provider_session_id TEXT,
            resume_input_id TEXT,
            provider_session_capture_method TEXT,
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT,
            row_version INTEGER NOT NULL DEFAULT 0
        );"#;

#[test]
fn invocations_schema_sql_unchanged_no_raw_io_columns_and_no_migration_surface() {
    let db_rs = include_str!("../src/db.rs");
    let schema_rs = include_str!("../src/schema.rs");
    let lib_rs = include_str!("../src/lib.rs");
    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");

    assert!(
        db_rs.contains(EXPECTED_INVOCATIONS_SCHEMA_SNIPPET),
        "AGE-129 must keep invocations_schema_sql unchanged and sidecar-based"
    );
    assert!(
        schema_rs.contains("pub const CURRENT_SCHEMA_VERSION: i32 = 6;"),
        "AGE-129 must not bump the StateDb schema version"
    );
    assert!(
        !lib_rs.contains("pub mod lifecycle_log"),
        "AGE-129 may re-export the lifecycle sink trait/no-op, but must not expose lifecycle_log as a public module"
    );
    assert!(
        lib_rs.contains("mod lifecycle_log;") || lib_rs.contains("pub(crate) mod lifecycle_log;"),
        "AGE-129 must add only a private lifecycle_log module declaration"
    );
    assert!(
        lib_rs.contains("LifecycleEventSink") && lib_rs.contains("NoopLifecycleEventSink"),
        "AGE-129 must publicly re-export only LifecycleEventSink and NoopLifecycleEventSink"
    );
    for private_helper in [
        "build_start_record",
        "build_start_error_record",
        "build_session_record",
        "build_session_error_record",
        "build_finalize_record",
        "build_finalize_error_record",
        "emit_and_forward",
    ] {
        assert!(
            !lib_rs.contains(private_helper),
            "AGE-129 builder helpers and emit_and_forward must remain crate-private implementation details"
        );
    }

    let mut migrations = std::fs::read_dir(migrations_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    migrations.sort();
    assert_eq!(
        migrations,
        vec![
            "0004_state_db_schema_boundary.sql",
            "0005_invocation_dual_session_ids.sql",
            "0006_age_58_dual_write_row_versions.sql",
        ],
        "AGE-129 must not add lifecycle/raw-io/event migrations"
    );
}
