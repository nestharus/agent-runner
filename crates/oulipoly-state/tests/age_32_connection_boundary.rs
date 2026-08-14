//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age_32_connection_boundary.rs
//!     role: intrinsic-surface
//!     Domain: state-db-connection-boundary-test-domain
//!     Owns:
//!       - StateDb public API source scanning and forbidden raw connection signature assertions
//!       - db.rs and db/*.rs include_str aggregation for boundary checks
//!       - StateDb raw connection and transaction compile-boundary assertions
//!       - tempfile::tempdir database fixture directory surface
//!       - session_replace source include_str write-transaction boundary check

#[test]
fn ti_39_state_db_public_api_has_no_raw_mutable_connection_escape() {
    let public_boundary_source = public_boundary_source();
    let opening_source = opening_source();

    assert_no_raw_mutable_connection_escape(&public_boundary_source);
    assert_no_forbidden_state_db_impl_escape(&public_boundary_source);
    assert_no_public_raw_connection(opening_source);
    assert_no_public_write_transaction(opening_source);
}

fn assert_no_raw_mutable_connection_escape(public_boundary_source: &str) {
    for forbidden in [
        "pub fn connection_mut",
        "pub fn into_connection",
        "pub fn raw_connection",
        "pub fn raw_connection_after_migration",
        "pub fn connection(&mut self)",
        "-> &mut Connection",
        "-> rusqlite::Connection",
        "-> Connection",
    ] {
        assert!(
            !public_boundary_source.contains(forbidden),
            "StateDb must not expose a raw writable connection; found forbidden signature fragment {forbidden}"
        );
    }
}

fn assert_no_forbidden_state_db_impl_escape(state_db_impl: &str) {
    for forbidden_method in ["fn into_connection", "fn connection_mut"] {
        assert!(
            !state_db_impl.contains(forbidden_method),
            "StateDb public surface must not define forbidden raw connection escape method {forbidden_method}"
        );
    }
}

fn assert_no_public_raw_connection(opening_source: &str) {
    assert!(
        opening_source.contains("pub fn connection(&self) -> StateReadConnection<'_>"),
        "StateDb must expose only the validated read projection"
    );
    assert!(
        opening_source.contains("pub(crate) fn raw_connection(&self)"),
        "StateDb internals still require crate-scoped persistence access"
    );
}

fn assert_no_public_write_transaction(opening_source: &str) {
    assert!(
        !opening_source.contains("pub fn with_write_txn"),
        "StateDb must not expose arbitrary closure-scoped write SQL"
    );
    assert!(
        !opening_source.contains("fn with_write_txn"),
        "arbitrary closure-scoped write SQL is no longer part of StateDb"
    );
}

#[test]
fn state_read_projection_rejects_dml_ddl_and_writable_schema_pragma() {
    let directory = tempfile::tempdir().unwrap();
    let state = oulipoly_state::StateDb::open(&directory.path().join("state.db")).unwrap();
    let connection = state.connection();

    for statement in [
        "DELETE FROM invocation_completion_obligations",
        "DROP TRIGGER trg_invocation_completion_obligations_append_only_delete",
        "PRAGMA writable_schema = ON",
        "PRAGMA busy_timeout(0)",
        "PRAGMA journal_mode(DELETE)",
        "PRAGMA user_version(0)",
    ] {
        assert!(
            matches!(
                connection.prepare(statement),
                Err(rusqlite::Error::InvalidQuery)
            ),
            "read projection accepted writable SQL: {statement}"
        );
    }
    connection
        .prepare("SELECT COUNT(*) FROM invocation_completion_obligations")
        .unwrap();
    connection.prepare("PRAGMA journal_mode").unwrap();
    connection
        .prepare("PRAGMA table_info(invocation_completion_obligations)")
        .unwrap();
}

fn public_boundary_source() -> String {
    format!("{}\n{}", db_source(), db_module_sources())
}

fn db_source() -> &'static str {
    include_str!("../src/db.rs")
}

fn opening_source() -> &'static str {
    concat!(
        include_str!("../src/db/opening_read_only.rs"),
        include_str!("../src/db/opening_write.rs"),
        include_str!("../src/db/opening_migrations.rs"),
    )
}

fn db_module_sources() -> &'static str {
    concat!(
        include_str!("../src/db/accounts.rs"),
        include_str!("../src/db/chain_backfill.rs"),
        include_str!("../src/db/chain_segments_compaction.rs"),
        include_str!("../src/db/chain_segments_import.rs"),
        include_str!("../src/db/chain_segments_open.rs"),
        include_str!("../src/db/cli_providers.rs"),
        include_str!("../src/db/discovered_models.rs"),
        include_str!("../src/db/discovery_types.rs"),
        include_str!("../src/db/invocation_artifacts.rs"),
        include_str!("../src/db/invocation_lifecycle_finalize.rs"),
        include_str!("../src/db/invocation_lifecycle_finalize_context.rs"),
        include_str!("../src/db/invocation_lifecycle_finalize_write.rs"),
        include_str!("../src/db/invocation_lifecycle_start.rs"),
        include_str!("../src/db/invocation_records.rs"),
        include_str!("../src/db/invocation_schema_legacy_migration.rs"),
        include_str!("../src/db/invocation_schema_projection.rs"),
        include_str!("../src/db/invocation_schema_repair.rs"),
        include_str!("../src/db/invocation_schema_session_turns.rs"),
        include_str!("../src/db/invocation_schema_table.rs"),
        include_str!("../src/db/invocation_window.rs"),
        include_str!("../src/db/lifecycle_invocation_row.rs"),
        include_str!("../src/db/lifecycle_log_adapter.rs"),
        include_str!("../src/db/model_parameters.rs"),
        include_str!("../src/db/opening_migrations.rs"),
        include_str!("../src/db/opening_read_only.rs"),
        include_str!("../src/db/opening_write.rs"),
        include_str!("../src/db/owned_turn_event_read.rs"),
        include_str!("../src/db/owned_turn_event_write.rs"),
        include_str!("../src/db/provider_quota_reads.rs"),
        include_str!("../src/db/provider_quota_refresh.rs"),
        include_str!("../src/db/provider_quota_status.rs"),
        include_str!("../src/db/provider_quota_test_support.rs"),
        include_str!("../src/db/provider_quota_window_writes.rs"),
        include_str!("../src/db/provider_quotas.rs"),
        include_str!("../src/db/provider_schema_migration.rs"),
        include_str!("../src/db/provider_schema_validation.rs"),
        include_str!("../src/db/provider_session_binding.rs"),
        include_str!("../src/db/providers.rs"),
        include_str!("../src/db/resume_active_segment.rs"),
        include_str!("../src/db/resume_lookup.rs"),
        include_str!("../src/db/resume_preview.rs"),
        include_str!("../src/db/resume_resolution.rs"),
        include_str!("../src/db/resume_types.rs"),
        include_str!("../src/db/returned_artifacts_codec.rs"),
        include_str!("../src/db/returned_artifacts_read.rs"),
        include_str!("../src/db/returned_artifacts_write.rs"),
        include_str!("../src/db/schema_types.rs"),
        include_str!("../src/db/session_capture.rs"),
        include_str!("../src/db/session_markers.rs"),
        include_str!("../src/db/session_turns_ingest.rs"),
        include_str!("../src/db/session_turns_query.rs"),
        include_str!("../src/db/session_turns_replace.rs"),
        include_str!("../src/db/sqlite_adapter.rs"),
        include_str!("../src/db/timestamps.rs"),
    )
}
