//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/migration_failure_tests.rs
//!     role: intrinsic-surface
//!     Domain: migration-failure-test-surface
//!     Owns:
//!       - crate::migrations Migration and MigrationError contract exercised by failure-path assertions
//! ```

use super::common::*;
use super::*;
#[test]
fn ti_04_ti_12_ti_24_ordered_migration_failure_rolls_back_and_reports_rebuild() {
    use crate::migrations::{self, Migration, MigrationError};

    let (target_version, id, sql) = failing_migration::failing_migration_parts();
    let failing = Migration {
        target_version,
        id,
        sql,
        post_sql_hook: None,
    };

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("state.db");
    let mut conn = sqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "
            PRAGMA user_version = 3;
            CREATE TABLE preserved_rows (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO preserved_rows (id, value) VALUES (1, 'before');
            ",
    )
    .unwrap();

    let err = migrations::run_with_db_path(&mut conn, &[&failing], db_path.clone()).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains(failing_migration::FAILING_MIGRATION_ID),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "target_version={}",
            failing_migration::FAILING_MIGRATION_TARGET_VERSION
        )),
        "{message}"
    );
    assert!(message.contains("agents migrate --rebuild"), "{message}");
    assert!(
        message.contains(&format!("db={}", db_path.display())),
        "{message}"
    );
    assert!(
        matches!(err, MigrationError::StepFailed { .. }),
        "expected StepFailed"
    );

    let version = sqlite_user_version(&conn);
    assert_eq!(version, 3);
    let preserved = preserved_row_value(&conn, 1);
    assert_eq!(preserved, "before");
    let marker_exists = sqlite_schema_object_count(&conn, "table", "age32_failure_marker");
    assert_eq!(marker_exists, 0, "failed migration left partial schema");
}
