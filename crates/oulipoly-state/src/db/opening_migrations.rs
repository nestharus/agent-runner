//! ## Declared roles
//!
//! - formatter
//! - validator
//! - orchestration
//! - predicate
//!
//! Role set: { formatter, validator, orchestration, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/opening_migrations.rs
//!     role: intrinsic-surface
//!     Domain: schema-version-gated-open-migration
//!     Owns:
//!       - CURRENT_SCHEMA_VERSION and MINIMUM_SUPPORTED_SCHEMA_VERSION schema-version bounds
//!       - SchemaCompatibility decision consumed to gate open-time migration
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION, SchemaCompatibility, migrations
//! ```
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
        provider_names: &LegacyProviderNames,
    ) -> Result<(), WritableOpenError> {
        match compatibility {
            SchemaCompatibility::Fresh => {
                Self::set_wal_mode(conn)?;
                Self::run_current_plan_from(path, conn, 0)
            }
            SchemaCompatibility::Current { .. } => Self::set_wal_mode(conn).map_err(Into::into),
            SchemaCompatibility::Migratable { stored } => {
                Self::set_wal_mode(conn)?;
                let stored = Self::promote_existing_dual_id_schema5_if_present(conn, stored)?;
                if stored < 27 && Self::legacy_invocations_shape_needs_pre_v27_rebuild(conn)? {
                    Self::run_migration_plan_through(path, conn, stored, 26)?;
                    Self::migrate_legacy_invocations_before_timestamp_contract(
                        conn,
                        provider_names,
                    )?;
                    Self::run_current_plan_from(path, conn, 26)
                } else {
                    Self::run_current_plan_from(path, conn, stored)
                }
            }
            SchemaCompatibility::LegacyVersionless => {
                Self::validate_versionless_shape(path, conn)?;
                let columns = Self::invocations_columns(conn)?;
                match Self::classify_invocations_schema(&columns) {
                    InvocationsSchemaShape::Empty | InvocationsSchemaShape::Current => {
                        Self::set_wal_mode(conn)?;
                        Self::run_current_plan_from(path, conn, MINIMUM_SUPPORTED_SCHEMA_VERSION)
                    }
                    InvocationsSchemaShape::LegacyPreUuid => {
                        Self::set_wal_mode(conn)?;
                        migrations::normalize_versionless_pre_uuid_baseline(
                            conn,
                            path.to_path_buf(),
                        )
                        .map_err(WritableOpenError::Migration)?;
                        Self::run_migration_plan_through(path, conn, 5, 26)?;
                        Self::migrate_legacy_invocations_before_timestamp_contract(
                            conn,
                            provider_names,
                        )?;
                        Self::run_current_plan_from(path, conn, 26)
                    }
                    InvocationsSchemaShape::UnrecognizedPreUuid(_) => {
                        Err(Self::unrecognized_versionless_error(path))
                    }
                }
            }
            SchemaCompatibility::Future { stored } => Err(Self::future_schema_error(path, stored)),
            SchemaCompatibility::UnrecognizedVersionless => {
                Err(Self::unrecognized_versionless_error(path))
            }
            SchemaCompatibility::Corrupt { reason } => {
                Err(Self::corrupt_schema_error(path, reason).into())
            }
        }
    }

    pub(super) fn run_current_plan_from(
        path: &Path,
        conn: &mut sqlite::Connection,
        stored: i32,
    ) -> Result<(), WritableOpenError> {
        let plan = migrations::current_plan_from(stored).map_err(WritableOpenError::Migration)?;
        migrations::run_with_db_path(conn, &plan, path.to_path_buf())
            .map_err(WritableOpenError::Migration)
    }

    fn run_migration_plan_through(
        path: &Path,
        conn: &mut sqlite::Connection,
        stored: i32,
        target: i32,
    ) -> Result<(), WritableOpenError> {
        let plan = migrations::plan(stored, target).map_err(WritableOpenError::Migration)?;
        migrations::run_with_db_path(conn, &plan, path.to_path_buf())
            .map_err(WritableOpenError::Migration)
    }

    fn legacy_invocations_shape_needs_pre_v27_rebuild(
        conn: &sqlite::Connection,
    ) -> Result<bool, String> {
        let columns = Self::invocations_columns(conn)?;
        Ok(Self::legacy_invocations_shape_is_pre_uuid(&columns))
    }

    pub(super) fn validate_versionless_shape(
        path: &Path,
        conn: &sqlite::Connection,
    ) -> Result<(), WritableOpenError> {
        if migrations::classify_versionless(conn)?.is_some() {
            Ok(())
        } else {
            Err(Self::unrecognized_versionless_error(path))
        }
    }

    pub(super) fn future_schema_error(path: &Path, stored: i32) -> WritableOpenError {
        WritableOpenError::Migration(migrations::MigrationError::Incompatible {
            db_path: path.to_path_buf(),
            stored,
            current: CURRENT_SCHEMA_VERSION,
        })
    }

    pub(super) fn unrecognized_versionless_error(path: &Path) -> WritableOpenError {
        WritableOpenError::Migration(migrations::MigrationError::UnrecognizedShape {
            db_path: path.to_path_buf(),
        })
    }

    pub(super) fn corrupt_schema_error(path: &Path, reason: String) -> String {
        format!(
            "Corrupt schema ({reason}); run `agents migrate --rebuild`. db={}",
            path.display()
        )
    }

    pub(super) fn apply_returned_artifacts_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(Self::format_returned_artifacts_schema_error)
    }

    fn format_returned_artifacts_schema_error(err: sqlite::Error) -> String {
        format!("Failed to ensure returned-artifacts schema: {err}")
    }

    pub(super) fn set_wal_mode(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(Self::format_wal_mode_error)
    }

    fn format_wal_mode_error(err: sqlite::Error) -> String {
        format!("Failed to set WAL mode: {err}; run `agents migrate --rebuild`")
    }
}
