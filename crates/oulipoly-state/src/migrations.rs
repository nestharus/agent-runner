use crate::schema::{self, CURRENT_SCHEMA_VERSION};
use rusqlite::Connection;
use std::fmt;
use std::path::PathBuf;

pub use crate::schema::classify;

#[derive(Debug)]
pub struct Migration {
    pub target_version: i32,
    pub id: &'static str,
    pub sql: &'static str,
}

static MIGRATIONS: &[Migration] = &[
    Migration {
        target_version: 4,
        id: "0004_state_db_schema_boundary",
        sql: include_str!("../migrations/0004_state_db_schema_boundary.sql"),
    },
    Migration {
        target_version: 5,
        id: "0005_invocation_dual_session_ids",
        sql: include_str!("../migrations/0005_invocation_dual_session_ids.sql"),
    },
];

pub fn manifest() -> &'static [Migration] {
    MIGRATIONS
}

pub fn plan(stored: i32, current: i32) -> Result<Vec<&'static Migration>, MigrationError> {
    if stored > current {
        return Err(MigrationError::Incompatible {
            db_path: PathBuf::from("<unknown>"),
            stored,
            current,
        });
    }

    Ok(manifest()
        .iter()
        .filter(|migration| {
            migration.target_version > stored && migration.target_version <= current
        })
        .collect())
}

#[allow(dead_code)]
pub(crate) fn run(conn: &mut Connection, plan: &[&Migration]) -> Result<(), MigrationError> {
    run_with_db_path(conn, plan, PathBuf::from("<memory>"))
}

pub(crate) fn run_with_db_path(
    conn: &mut Connection,
    plan: &[&Migration],
    db_path: PathBuf,
) -> Result<(), MigrationError> {
    for migration in plan {
        if let Err(source) = run_step(conn, migration) {
            return Err(MigrationError::StepFailed {
                db_path,
                id: migration.id,
                target_version: migration.target_version,
                source,
            });
        }
    }
    Ok(())
}

fn run_step(conn: &mut Connection, migration: &Migration) -> Result<(), rusqlite::Error> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let result = conn.execute_batch(migration.sql).and_then(|_| {
        conn.pragma_update(None, "user_version", migration.target_version)?;
        conn.execute_batch("COMMIT;")
    });
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK;");
    }
    result
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
            } => write!(
                f,
                "schema is incompatible (stored={stored}, current={current}); run `agents migrate --rebuild`. db={}",
                db_path.display()
            ),
            MigrationError::StepFailed {
                db_path,
                id,
                target_version,
                source,
            } => write!(
                f,
                "migration step {id} failed at target_version={target_version}: {source}; run `agents migrate --rebuild`. db={}",
                db_path.display()
            ),
            MigrationError::UnrecognizedShape { db_path } => write!(
                f,
                "unrecognized schema shape; run `agents migrate --rebuild`. db={}",
                db_path.display()
            ),
        }
    }
}

impl std::error::Error for MigrationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MigrationError::StepFailed { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn current_plan_from(stored: i32) -> Result<Vec<&'static Migration>, MigrationError> {
    plan(stored, CURRENT_SCHEMA_VERSION)
}
