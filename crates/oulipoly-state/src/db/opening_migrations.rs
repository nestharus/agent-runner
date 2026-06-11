//! ## Declared roles
//!
//! - formatter
//! - validator
//! - orchestration
//! - predicate
//!
//! Role set: { formatter, validator, orchestration, predicate }
//!
//! State database open-time migration dispatch and returned-artifact validator repair.

use super::*;
use crate::migrations;
use crate::schema::{
    CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION, SchemaCompatibility,
};

impl StateDb {
    pub(super) fn compatibility_runs_open_migrations(compatibility: &SchemaCompatibility) -> bool {
        matches!(
            compatibility,
            SchemaCompatibility::Fresh
                | SchemaCompatibility::Migratable { .. }
                | SchemaCompatibility::LegacyVersionless
        )
    }

    pub(super) fn dispatch_open_migration_plan(
        path: &Path,
        conn: &mut sqlite::Connection,
        compatibility: SchemaCompatibility,
    ) -> Result<(), String> {
        match compatibility {
            SchemaCompatibility::Fresh => {
                Self::set_wal_mode(conn)?;
                Self::run_current_plan_from(path, conn, 0)
            }
            SchemaCompatibility::Current { .. } => Self::set_wal_mode(conn),
            SchemaCompatibility::Migratable { stored } => {
                Self::set_wal_mode(conn)?;
                let stored = Self::promote_existing_dual_id_schema5_if_present(conn, stored)?;
                Self::run_current_plan_from(path, conn, stored)
            }
            SchemaCompatibility::LegacyVersionless => {
                Self::validate_versionless_shape(path, conn)?;
                Self::set_wal_mode(conn)?;
                Self::run_current_plan_from(path, conn, MINIMUM_SUPPORTED_SCHEMA_VERSION)
            }
            SchemaCompatibility::Future { stored } => Err(Self::future_schema_error(path, stored)),
            SchemaCompatibility::UnrecognizedVersionless => {
                Err(Self::unrecognized_versionless_error(path))
            }
            SchemaCompatibility::Corrupt { reason } => {
                Err(Self::corrupt_schema_error(path, reason))
            }
        }
    }

    pub(super) fn run_current_plan_from(
        path: &Path,
        conn: &mut sqlite::Connection,
        stored: i32,
    ) -> Result<(), String> {
        let plan = migrations::current_plan_from(stored).map_err(|e| e.to_string())?;
        migrations::run_with_db_path(conn, &plan, path.to_path_buf()).map_err(|e| e.to_string())
    }

    pub(super) fn validate_versionless_shape(
        path: &Path,
        conn: &sqlite::Connection,
    ) -> Result<(), String> {
        if migrations::classify_versionless(conn)?.is_some() {
            Ok(())
        } else {
            Err(Self::unrecognized_versionless_error(path))
        }
    }

    pub(super) fn future_schema_error(path: &Path, stored: i32) -> String {
        migrations::MigrationError::Incompatible {
            db_path: path.to_path_buf(),
            stored,
            current: CURRENT_SCHEMA_VERSION,
        }
        .to_string()
    }

    pub(super) fn unrecognized_versionless_error(path: &Path) -> String {
        migrations::MigrationError::UnrecognizedShape {
            db_path: path.to_path_buf(),
        }
        .to_string()
    }

    pub(super) fn corrupt_schema_error(path: &Path, reason: String) -> String {
        format!(
            "Corrupt schema ({reason}); run `agents migrate --rebuild`. db={}",
            path.display()
        )
    }

    pub(super) fn apply_returned_artifacts_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(|e| format!("Failed to ensure returned-artifacts schema: {e}"))
    }

    pub(super) fn set_wal_mode(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to set WAL mode: {e}; run `agents migrate --rebuild`"))
    }
}
