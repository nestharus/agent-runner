//! ## Declared roles
//! orchestration, mapper, validator, formatter
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age_149_migration_error_characterization.rs
//!     role: intrinsic-surface
//!     Domain: state-db-migration-error-characterization-test-domain
//!     Owns:
//!       - oulipoly_state::migrations MigrationError source-chain characterization
//!       - oulipoly_state::migrations StepFailed display context characterization
//!       - fixtures::failing_migration failing SQL migration fixture

mod fixtures;

use fixtures::failing_migration;
use oulipoly_state::migrations::{self, Migration};
use rusqlite::Connection;
use std::error::Error;

#[test]
fn idx_mig_05_step_failed_preserves_sqlite_source_and_display_context() {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("state.db");
    let mut conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("PRAGMA user_version = 3;").unwrap();

    let (target_version, id, sql) = failing_migration::failing_migration_parts();
    let failing = Migration {
        target_version,
        id,
        sql,
        post_sql_hook: None,
    };

    let err = migrations::run_with_db_path(&mut conn, &[&failing], db_path.clone()).unwrap_err();

    let source = err
        .source()
        .expect("failed SQL migration must expose its rusqlite source");
    let sqlite_source = source
        .downcast_ref::<rusqlite::Error>()
        .expect("migration source must remain rusqlite::Error");
    match sqlite_source {
        rusqlite::Error::SqliteFailure(_, Some(message)) => {
            assert!(
                message.contains("definitely_missing_age32_table"),
                "unexpected SQLite failure message: {message}"
            );
        }
        other => panic!("expected SqliteFailure with message, got {other:?}"),
    }

    let display = err.to_string();
    assert!(
        display.contains(failing_migration::FAILING_MIGRATION_ID),
        "{display}"
    );
    assert!(
        display.contains(&format!(
            "target_version={}",
            failing_migration::FAILING_MIGRATION_TARGET_VERSION
        )),
        "{display}"
    );
    assert!(display.contains("agents migrate --rebuild"), "{display}");
    assert!(
        display.contains(&format!("db={}", db_path.display())),
        "{display}"
    );
}
