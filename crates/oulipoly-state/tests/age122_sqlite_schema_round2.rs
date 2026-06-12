//! ## Declared roles
//!
//! - validator
//! - accessor
//! - formatter
//! - mapper
//! - filter
//!
//! Role set: { validator, accessor, formatter, mapper, filter }
//!
//! Schema-boundary regression assertions over source snippets and migration files.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age122_sqlite_schema_round2.rs
//!     role: intrinsic-surface
//!     Domain: sqlite-schema-boundary-source-scan-test-domain
//!     Owns:
//!       - include_str source snippets for db invocation schema, schema.rs, and lib.rs
//!       - migration directory read and migration filename sorting surface
//!       - std::fs::{read_dir, DirEntry, ReadDir} migration-file enumeration support surface
//!       - std::ffi::OsString migration filename formatting support surface
//!       - std::path::{Path, PathBuf} migration-directory support surface
//! ```

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
            provider_session_resolved_account TEXT,
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT,
            row_version INTEGER NOT NULL DEFAULT 0
        );"#;

#[test]
fn invocations_schema_sql_unchanged_no_raw_io_columns_and_no_migration_surface() {
    assert!(
        invocation_schema_source().contains(EXPECTED_INVOCATIONS_SCHEMA_SNIPPET),
        "AGE-129 must keep invocations_schema_sql unchanged and sidecar-based"
    );
    assert!(
        schema_source().contains("pub const CURRENT_SCHEMA_VERSION: i32 = 9;"),
        "AGE-129 must not bump the StateDb schema version beyond the AGE-163 v9 baseline"
    );
    assert!(
        !lib_source().contains("pub mod lifecycle_log"),
        "AGE-129 may re-export the lifecycle sink trait/no-op, but must not expose lifecycle_log as a public module"
    );
    assert!(
        lib_source().contains("mod lifecycle_log;")
            || lib_source().contains("pub(crate) mod lifecycle_log;"),
        "AGE-129 must add only a private lifecycle_log module declaration"
    );
    assert!(
        lib_source().contains("LifecycleEventSink")
            && lib_source().contains("NoopLifecycleEventSink"),
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
            !lib_source().contains(private_helper),
            "AGE-129 builder helpers and emit_and_forward must remain crate-private implementation details"
        );
    }

    assert_eq!(
        sorted_migration_names(),
        vec![
            "0004_state_db_schema_boundary.sql",
            "0005_invocation_dual_session_ids.sql",
            "0006_age_58_dual_write_row_versions.sql",
            "0007_age_123_resume_provider_identity.sql",
            "0008_owned_turn_events.sql",
            "0009_age163_working_set_and_round_robin.sql",
        ],
        "AGE-129 must not add lifecycle/raw-io/event migrations beyond the AGE-163 v9 baseline"
    );
}

fn invocation_schema_source() -> &'static str {
    concat!(
        include_str!("../src/db/invocation_schema_table.rs"),
        include_str!("../src/db/invocation_schema_repair.rs"),
        include_str!("../src/db/invocation_schema_projection.rs"),
        include_str!("../src/db/invocation_schema_session_turns.rs"),
        include_str!("../src/db/invocation_schema_legacy_migration.rs"),
    )
}

fn schema_source() -> &'static str {
    include_str!("../src/schema.rs")
}

fn lib_source() -> &'static str {
    include_str!("../src/lib.rs")
}

fn sorted_migration_names() -> Vec<String> {
    let mut migrations = migration_names();
    migrations.sort();
    migrations
}

fn migration_names() -> Vec<String> {
    migration_entries()
        .into_iter()
        .map(migration_name)
        .collect()
}

fn migration_entries() -> Vec<std::fs::DirEntry> {
    collect_migration_entries(require_migration_read_dir(read_migration_dir()))
}

fn read_migration_dir() -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(migrations_dir())
}

fn require_migration_read_dir(result: std::io::Result<std::fs::ReadDir>) -> std::fs::ReadDir {
    result.unwrap()
}

fn collect_migration_entries(entries: std::fs::ReadDir) -> Vec<std::fs::DirEntry> {
    entries.map(require_migration_entry).collect()
}

fn require_migration_entry(result: std::io::Result<std::fs::DirEntry>) -> std::fs::DirEntry {
    result.unwrap()
}

fn migration_name(entry: std::fs::DirEntry) -> String {
    format_migration_file_name(migration_file_name(entry))
}

fn migration_file_name(entry: std::fs::DirEntry) -> std::ffi::OsString {
    entry.file_name()
}

fn format_migration_file_name(name: std::ffi::OsString) -> String {
    name.to_string_lossy().into_owned()
}

fn migrations_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations")
}
