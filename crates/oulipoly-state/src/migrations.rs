//! ## Declared roles
//! accessor, filter, validator, predicate, orchestration, formatter, mapper

use crate::schema::{self, CURRENT_SCHEMA_VERSION};
use rusqlite::Connection;
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
];

pub fn manifest() -> &'static [Migration] {
    MIGRATIONS
}

pub fn plan(stored: i32, current: i32) -> Result<Vec<&'static Migration>, MigrationError> {
    validate_version_range(stored, current)?;

    Ok(migrations_between(stored, current))
}

fn validate_version_range(stored: i32, current: i32) -> Result<(), MigrationError> {
    if stored > current {
        return Err(incompatible_version_error(
            PathBuf::from("<unknown>"),
            stored,
            current,
        ));
    }
    Ok(())
}

fn incompatible_version_error(db_path: PathBuf, stored: i32, current: i32) -> MigrationError {
    MigrationError::Incompatible {
        db_path,
        stored,
        current,
    }
}

fn migrations_between(stored: i32, current: i32) -> Vec<&'static Migration> {
    manifest()
        .iter()
        .filter(|migration| migration_is_in_range(migration, stored, current))
        .collect()
}

fn migration_is_in_range(migration: &Migration, stored: i32, current: i32) -> bool {
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
    for migration in plan {
        run_planned_step(conn, migration, db_path.clone())?;
    }
    Ok(())
}

fn run_planned_step(
    conn: &mut Connection,
    migration: &Migration,
    db_path: PathBuf,
) -> Result<(), MigrationError> {
    run_step(conn, migration).map_err(|source| step_failed_error(db_path, migration, source))
}

fn step_failed_error(
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

fn run_step(conn: &mut Connection, migration: &Migration) -> Result<(), rusqlite::Error> {
    begin_migration_transaction(conn)?;
    let result = apply_migration_step(conn, migration).and_then(|_| commit_migration(conn));
    if migration_step_failed(&result) {
        rollback_migration(conn);
    }
    result
}

fn migration_step_failed(result: &Result<(), rusqlite::Error>) -> bool {
    result.is_err()
}

fn begin_migration_transaction(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("BEGIN IMMEDIATE;")
}

fn apply_migration_step(
    conn: &mut Connection,
    migration: &Migration,
) -> Result<(), rusqlite::Error> {
    conn.execute_batch(migration.sql)?;
    update_user_version(conn, migration.target_version)?;
    run_post_sql_hook(conn, migration.post_sql_hook)
}

fn update_user_version(conn: &mut Connection, target_version: i32) -> Result<(), rusqlite::Error> {
    conn.pragma_update(None, "user_version", target_version)
}

fn run_post_sql_hook(
    conn: &Connection,
    post_sql_hook: Option<PostSqlHook>,
) -> Result<(), rusqlite::Error> {
    if let Some(post_sql_hook) = post_sql_hook {
        return post_sql_hook(conn);
    }
    Ok(())
}

fn commit_migration(conn: &mut Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("COMMIT;")
}

fn rollback_migration(conn: &mut Connection) {
    let _ = conn.execute_batch("ROLLBACK;");
}

fn apply_v7_resume_provider_identity(conn: &Connection) -> Result<(), rusqlite::Error> {
    if !table_exists(conn, "invocations")?
        || column_exists(conn, "invocations", "provider_session_resolved_account")?
    {
        return Ok(());
    }

    conn.execute_batch("ALTER TABLE invocations ADD COLUMN provider_session_resolved_account TEXT;")
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, rusqlite::Error> {
    conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM sqlite_schema
              WHERE type = 'table' AND name = ?1
         )",
        [table],
        |row| row.get::<_, bool>(0),
    )
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in columns {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn classify_versionless(
    conn: &Connection,
) -> Result<Option<&'static str>, crate::StateDbError> {
    schema::classify_versionless(conn)
}

#[derive(Debug)]
pub enum MigrationError {
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
        match self {
            MigrationError::Incompatible {
                db_path,
                stored,
                current,
            } => f.write_str(&format_incompatible_error(db_path, *stored, *current)),
            MigrationError::StepFailed {
                db_path,
                id,
                target_version,
                source,
            } => f.write_str(&format_step_failed_error(
                db_path,
                id,
                *target_version,
                source,
            )),
            MigrationError::UnrecognizedShape { db_path } => {
                f.write_str(&format_unrecognized_shape_error(db_path))
            }
        }
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
        match self {
            MigrationError::StepFailed { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn current_plan_from(stored: i32) -> Result<Vec<&'static Migration>, MigrationError> {
    plan(stored, CURRENT_SCHEMA_VERSION)
}
