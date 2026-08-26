//! ## Declared roles
//! accessor, filter, formatter, mapper, orchestration, predicate, validator

use crate::schema::{self, CURRENT_SCHEMA_VERSION};
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use std::fmt;
use std::path::{Path, PathBuf};

pub use crate::schema::classify;

#[derive(Debug)]
pub struct Migration {
    pub target_version: i32,
    pub id: &'static str,
    pub sql: &'static str,
    pub post_sql_hook: Option<PostSqlHook>,
}

pub type PostSqlHook = fn(&Connection) -> Result<(), rusqlite::Error>;

static MIGRATIONS: &[Migration] = &[
    Migration {
        target_version: 4,
        id: "0004_state_db_schema_boundary",
        sql: include_str!("../migrations/0004_state_db_schema_boundary.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 5,
        id: "0005_invocation_dual_session_ids",
        sql: include_str!("../migrations/0005_invocation_dual_session_ids.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 6,
        id: "0006_age_58_dual_write_row_versions",
        sql: include_str!("../migrations/0006_age_58_dual_write_row_versions.sql"),
        post_sql_hook: Some(crate::deployment::row_version::migrate_v6::apply_v6_row_version),
    },
    Migration {
        target_version: 7,
        id: "0007_age_123_resume_provider_identity",
        sql: include_str!("../migrations/0007_age_123_resume_provider_identity.sql"),
        post_sql_hook: Some(apply_v7_resume_provider_identity),
    },
    Migration {
        target_version: 8,
        id: "0008_owned_turn_events",
        sql: include_str!("../migrations/0008_owned_turn_events.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 9,
        id: "0009_age163_working_set_and_round_robin",
        sql: include_str!("../migrations/0009_age163_working_set_and_round_robin.sql"),
        post_sql_hook: Some(apply_v9_working_set_columns),
    },
    Migration {
        target_version: 10,
        id: "0010_imported_session_display_metadata",
        sql: include_str!("../migrations/0010_imported_session_display_metadata.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 11,
        id: "0011_durable_session_lifecycle",
        sql: include_str!("../migrations/0011_durable_session_lifecycle.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 12,
        id: "0012_session_ingress_evidence",
        sql: include_str!("../migrations/0012_session_ingress_evidence.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 13,
        id: "0013_fresh_continuations",
        sql: include_str!("../migrations/0013_fresh_continuations.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 14,
        id: "0014_invocation_completion_obligations",
        sql: include_str!("../migrations/0014_invocation_completion_obligations.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 15,
        id: "0015_invocation_completion_continuity",
        sql: include_str!("../migrations/0015_invocation_completion_continuity.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 16,
        id: "0016_invocation_completion_authority_summary",
        sql: include_str!("../migrations/0016_invocation_completion_authority_summary.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 17,
        id: "0017_completion_registration_authority",
        sql: include_str!("../migrations/0017_completion_registration_authority.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 18,
        id: "0018_invocation_completion_materialization_summary",
        sql: include_str!("../migrations/0018_invocation_completion_materialization_summary.sql"),
        post_sql_hook: None,
    },
    Migration {
        target_version: 19,
        id: "0019_invocation_running_projection_index",
        sql: include_str!("../migrations/0019_invocation_running_projection_index.sql"),
        post_sql_hook: Some(apply_v19_invocation_running_projection_index),
    },
];

pub fn manifest() -> &'static [Migration] {
    MIGRATIONS
}

pub fn plan(stored: i32, current: i32) -> Result<Vec<&'static Migration>, MigrationError> {
    validate_plan_version_window(stored, current)?;

    Ok(filter_manifest_by_version_window(stored, current))
}

fn validate_plan_version_window(stored: i32, current: i32) -> Result<(), MigrationError> {
    if stored_version_is_newer_than_current(stored, current) {
        return Err(map_incompatible_version_error(
            PathBuf::from("<unknown>"),
            stored,
            current,
        ));
    }
    Ok(())
}

fn stored_version_is_newer_than_current(stored: i32, current: i32) -> bool {
    stored > current
}

fn map_incompatible_version_error(db_path: PathBuf, stored: i32, current: i32) -> MigrationError {
    MigrationError::Incompatible {
        db_path,
        stored,
        current,
    }
}

fn filter_manifest_by_version_window(stored: i32, current: i32) -> Vec<&'static Migration> {
    manifest()
        .iter()
        .filter(|migration| migration_is_missing_forward_step(migration, stored, current))
        .collect()
}

fn migration_is_missing_forward_step(migration: &Migration, stored: i32, current: i32) -> bool {
    migration.target_version > stored && migration.target_version <= current
}

#[allow(dead_code)]
pub(crate) fn run(conn: &mut Connection, plan: &[&Migration]) -> Result<(), MigrationError> {
    run_with_db_path(conn, plan, PathBuf::from("<memory>"))
}

pub fn run_with_db_path(
    conn: &mut Connection,
    plan: &[&Migration],
    db_path: PathBuf,
) -> Result<(), MigrationError> {
    register_connection_primitives(conn)
        .map_err(|source| map_primitive_registration_error(db_path.clone(), source))?;
    for migration in plan {
        run_planned_step_with_path(conn, migration, db_path.clone())?;
    }
    Ok(())
}

pub(crate) fn register_connection_primitives(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.create_scalar_function(
        "oulipoly_utf8_text",
        1,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |context| {
            Ok(matches!(
                context.get_raw(0),
                ValueRef::Text(bytes) if std::str::from_utf8(bytes).is_ok()
            ))
        },
    )
}

fn apply_v19_invocation_running_projection_index(conn: &Connection) -> Result<(), rusqlite::Error> {
    let mut statement = conn.prepare("PRAGMA table_info(invocations)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if ["parent_invocation_id", "status", "created_at"]
        .iter()
        .all(|required| columns.iter().any(|column| column == required))
    {
        conn.execute_batch(crate::db::invocation_running_projection_index_sql())?;
    }
    Ok(())
}

fn map_primitive_registration_error(db_path: PathBuf, source: rusqlite::Error) -> MigrationError {
    MigrationError::PrimitiveRegistrationFailed { db_path, source }
}

fn run_planned_step_with_path(
    conn: &mut Connection,
    migration: &Migration,
    db_path: PathBuf,
) -> Result<(), MigrationError> {
    run_step_transaction(conn, migration)
        .map_err(|source| map_step_failed_error(db_path, migration, source))
}

fn map_step_failed_error(
    db_path: PathBuf,
    migration: &Migration,
    source: rusqlite::Error,
) -> MigrationError {
    MigrationError::StepFailed {
        db_path,
        id: migration.id,
        target_version: migration.target_version,
        source,
    }
}

fn run_step_transaction(
    conn: &mut Connection,
    migration: &Migration,
) -> Result<(), rusqlite::Error> {
    begin_migration_transaction(conn)?;
    let result = complete_migration_step(conn, migration);
    finalize_migration_transaction(conn, result)
}

fn begin_migration_transaction(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    execute_migration_transaction_sql(conn, "BEGIN IMMEDIATE;")
}

fn complete_migration_step(
    conn: &mut Connection,
    migration: &Migration,
) -> Result<(), rusqlite::Error> {
    apply_migration_step_body(conn, migration)?;
    commit_migration_transaction(conn)
}

fn apply_migration_step_body(
    conn: &mut Connection,
    migration: &Migration,
) -> Result<(), rusqlite::Error> {
    apply_migration_sql(conn, migration.sql)?;
    record_migration_target_version(conn, migration.target_version)?;
    dispatch_post_sql_hook(conn, migration.post_sql_hook)
}

fn apply_migration_sql(conn: &mut Connection, sql: &str) -> Result<(), rusqlite::Error> {
    conn.execute_batch(sql)
}

fn record_migration_target_version(
    conn: &mut Connection,
    target_version: i32,
) -> Result<(), rusqlite::Error> {
    conn.pragma_update(None, "user_version", target_version)
}

fn dispatch_post_sql_hook(
    conn: &Connection,
    post_sql_hook: Option<PostSqlHook>,
) -> Result<(), rusqlite::Error> {
    if let Some(post_sql_hook) = post_sql_hook {
        return post_sql_hook(conn);
    }
    Ok(())
}

fn commit_migration_transaction(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    execute_migration_transaction_sql(conn, "COMMIT;")
}

fn finalize_migration_transaction(
    conn: &mut Connection,
    result: Result<(), rusqlite::Error>,
) -> Result<(), rusqlite::Error> {
    if migration_step_failed(&result) {
        rollback_migration_transaction(conn);
    }

    result
}

fn migration_step_failed(result: &Result<(), rusqlite::Error>) -> bool {
    result.is_err()
}

fn rollback_migration_transaction(conn: &mut Connection) {
    let _ = execute_migration_transaction_sql(conn, "ROLLBACK;");
}

fn execute_migration_transaction_sql(
    conn: &mut Connection,
    sql: &str,
) -> Result<(), rusqlite::Error> {
    conn.execute_batch(sql)
}

fn apply_v7_resume_provider_identity(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "invocations")?
        || column_exists(conn, "invocations", "provider_session_resolved_account")?
    {
        return Ok(());
    }

    conn.execute_batch("ALTER TABLE invocations ADD COLUMN provider_session_resolved_account TEXT;")
}

fn apply_v9_working_set_columns(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "provider_quotas")? {
        return Ok(());
    }
    add_column_if_missing(
        conn,
        "provider_quotas",
        "next_available_at",
        "ALTER TABLE provider_quotas ADD COLUMN next_available_at TEXT NULL",
    )?;
    add_column_if_missing(
        conn,
        "provider_quotas",
        "last_refresh_at",
        "ALTER TABLE provider_quotas ADD COLUMN last_refresh_at TEXT NULL",
    )?;
    add_column_if_missing(
        conn,
        "provider_quotas",
        "failure_class",
        "ALTER TABLE provider_quotas ADD COLUMN failure_class TEXT NULL",
    )?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    sql: &str,
) -> Result<(), rusqlite::Error> {
    if column_exists(conn, table, column)? {
        return Ok(());
    }
    conn.execute_batch(sql)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, rusqlite::Error> {
    let exists = read_table_exists_scalar(conn, table)?;
    Ok(schema_match_exists(exists))
}

fn read_table_exists_scalar(conn: &Connection, table: &str) -> Result<i64, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM sqlite_schema
              WHERE type = 'table' AND name = ?1
         )",
        [table],
        |row| row.get(0),
    )
}

fn schema_match_exists(value: i64) -> bool {
    value != 0
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    let sql = format_table_info_query(table);
    let columns = read_column_names(conn, &sql)?;
    Ok(contains_column(&columns, column))
}

fn format_table_info_query(table: &str) -> String {
    format!("PRAGMA table_info({table})")
}

fn read_column_names(conn: &Connection, sql: &str) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = conn.prepare(sql)?;
    let columns = stmt.query_map([], |row| row.get(1))?;
    columns.collect()
}

fn contains_column(columns: &[String], column: &str) -> bool {
    columns.iter().any(|name| name == column)
}

pub(crate) fn classify_versionless(
    conn: &Connection,
) -> Result<Option<&'static str>, crate::StateDbError> {
    schema::classify_versionless(conn)
}

#[derive(Debug)]
pub enum MigrationError {
    PrimitiveRegistrationFailed {
        db_path: PathBuf,
        source: rusqlite::Error,
    },
    Incompatible {
        db_path: PathBuf,
        stored: i32,
        current: i32,
    },
    StepFailed {
        db_path: PathBuf,
        id: &'static str,
        target_version: i32,
        source: rusqlite::Error,
    },
    UnrecognizedShape {
        db_path: PathBuf,
    },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_migration_error(self))
    }
}

fn format_migration_error(error: &MigrationError) -> String {
    match error {
        MigrationError::PrimitiveRegistrationFailed { db_path, source } => format!(
            "failed to register state DB migration primitives: {source}; db={}",
            db_path.display()
        ),
        MigrationError::Incompatible {
            db_path,
            stored,
            current,
        } => format_incompatible_error(db_path, *stored, *current),
        MigrationError::StepFailed {
            db_path,
            id,
            target_version,
            source,
        } => format_step_failed_error(db_path, id, *target_version, source),
        MigrationError::UnrecognizedShape { db_path } => format_unrecognized_shape_error(db_path),
    }
}

fn format_incompatible_error(db_path: &Path, stored: i32, current: i32) -> String {
    format!(
        "schema is incompatible (stored={stored}, current={current}); run `agents migrate --rebuild`. db={}",
        db_path.display()
    )
}

fn format_step_failed_error(
    db_path: &Path,
    id: &str,
    target_version: i32,
    source: &rusqlite::Error,
) -> String {
    format!(
        "migration step {id} failed at target_version={target_version}: {source}; run `agents migrate --rebuild`. db={}",
        db_path.display()
    )
}

fn format_unrecognized_shape_error(db_path: &Path) -> String {
    format!(
        "unrecognized schema shape; run `agents migrate --rebuild`. db={}",
        db_path.display()
    )
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        map_migration_error_source(self)
    }
}

fn map_migration_error_source(
    error: &MigrationError,
) -> Option<&(dyn std::error::Error + 'static)> {
    match error {
        MigrationError::PrimitiveRegistrationFailed { source, .. }
        | MigrationError::StepFailed { source, .. } => Some(source),
        _ => None,
    }
}

pub fn current_plan_from(stored: i32) -> Result<Vec<&'static Migration>, MigrationError> {
    plan(stored, CURRENT_SCHEMA_VERSION)
}
