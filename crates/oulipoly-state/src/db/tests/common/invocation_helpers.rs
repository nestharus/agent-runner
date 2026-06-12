//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - validator
//!
//! Role set: { accessor, formatter, mapper, orchestration, validator }

use super::super::*;
pub(in crate::db::tests) fn insert_invocation_fixture(
    db: &StateDb,
    invocation_uuid: &str,
    parent_invocation_id: Option<i64>,
    created_at: &str,
) -> i64 {
    let start = fixture_invocation_start(invocation_uuid, parent_invocation_id);
    let id = start_fixture_invocation(db, &start);
    set_invocation_created_at(db, id, created_at);
    id
}

fn fixture_invocation_start(
    invocation_uuid: &str,
    parent_invocation_id: Option<i64>,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation_uuid.to_string(),
        model_name: "fixture-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id,
    }
}

fn start_fixture_invocation(db: &StateDb, start: &InvocationStart) -> i64 {
    db.start_invocation(start).unwrap()
}

fn set_invocation_created_at(db: &StateDb, id: i64, created_at: &str) {
    db.conn
        .execute(
            "UPDATE invocations SET created_at = ?1 WHERE id = ?2",
            sqlite::params![created_at, id],
        )
        .unwrap();
}

pub(in crate::db::tests) fn seed_running_invocation(db: &StateDb) -> i64 {
    let start = running_invocation_start();
    db.start_invocation(&start).unwrap()
}

fn running_invocation_start() -> InvocationStart {
    InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

pub(in crate::db::tests) type InvocationMigrationBackfillRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

pub(in crate::db::tests) type InvocationDualIdRow =
    (String, Option<String>, Option<String>, Option<String>);

pub(in crate::db::tests) type InvocationSessionBindingRow =
    (Option<String>, Option<String>, Option<String>);

pub(in crate::db::tests) fn legacy_invocation_uuid(db: &StateDb) -> String {
    db.conn
        .query_row(
            "SELECT invocation_uuid FROM invocations LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_migration_backfill_row(
    db: &StateDb,
    invocation_uuid: &str,
) -> InvocationMigrationBackfillRow {
    db.conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method, terminal_reason, status, error_category
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_dual_id_row(
    db: &StateDb,
    invocation_uuid: &str,
) -> InvocationDualIdRow {
    db.conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_provider_session_id(
    db: &StateDb,
    id: i64,
) -> Option<String> {
    db.conn
        .query_row(
            "SELECT provider_session_id FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_provider_and_resume_input_ids(
    db: &StateDb,
    id: i64,
) -> (Option<String>, Option<String>) {
    db.conn
        .query_row(
            "SELECT provider_session_id, resume_input_id FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_session_provider_resume_ids(
    db: &StateDb,
    id: i64,
) -> InvocationSessionBindingRow {
    db.conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id
                 FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_capture_projection(
    db: &StateDb,
    id: i64,
) -> InvocationSessionBindingRow {
    db.conn
        .query_row(
            "SELECT provider_session_id, resume_input_id, provider_session_capture_method
                 FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_session_id(db: &StateDb, id: i64) -> Option<String> {
    db.conn
        .query_row(
            "SELECT session_id FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_terminal_reason_for_model(
    db: &StateDb,
    model_name: &str,
) -> Option<String> {
    db.conn
        .query_row(
            "SELECT terminal_reason FROM invocations WHERE model_name = ?1",
            sqlite::params![model_name],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_row_count(conn: &sqlite::Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

pub(in crate::db::tests) fn record_provider_invocation(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
    success: bool,
    error_category: Option<&str>,
    stderr_snippet: Option<&str>,
) -> i64 {
    let start = provider_invocation_start(model_name, provider_name, provider_index);
    let id = start_fixture_invocation(db, &start);
    finalize_provider_invocation(db, id, success, error_category, stderr_snippet);
    id
}

fn provider_invocation_start(
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id: None,
    }
}

fn finalize_provider_invocation(
    db: &StateDb,
    id: i64,
    success: bool,
    error_category: Option<&str>,
    stderr_snippet: Option<&str>,
) {
    db.finalize_invocation(
        id,
        success,
        provider_invocation_exit_code(success),
        error_category,
        stderr_snippet,
    )
    .unwrap();
}

fn provider_invocation_exit_code(success: bool) -> i32 {
    if success { 0 } else { 1 }
}

pub(in crate::db::tests) fn with_models_config(model_name: &str, body: &str, test: impl FnOnce()) {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    write_model_config_fixture(dir.path(), model_name, body);

    let old = current_xdg_config_home();
    isolate_xdg_config_home(dir.path());
    let result = run_models_config_test(test);
    restore_xdg_config_home(old);
    resume_models_config_panic(result);
}

fn write_model_config_fixture(dir: &std::path::Path, model_name: &str, body: &str) {
    let models_dir = models_fixture_dir(dir);
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(model_fixture_path(&models_dir, model_name), body).unwrap();
}

fn models_fixture_dir(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("oulipoly-agent-runner").join("models")
}

fn model_fixture_path(models_dir: &std::path::Path, model_name: &str) -> std::path::PathBuf {
    models_dir.join(model_fixture_filename(model_name))
}

fn model_fixture_filename(model_name: &str) -> String {
    format!("{model_name}.toml")
}

fn current_xdg_config_home() -> Option<std::ffi::OsString> {
    std::env::var_os("XDG_CONFIG_HOME")
}

fn isolate_xdg_config_home(dir: &std::path::Path) {
    // Tests need to isolate config-driven provider-name resolution.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir);
    }
}

fn run_models_config_test(test: impl FnOnce()) -> std::thread::Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(test))
}

fn restore_xdg_config_home(old: Option<std::ffi::OsString>) {
    match old {
        Some(value) => unsafe {
            std::env::set_var("XDG_CONFIG_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        },
    }
}

fn resume_models_config_panic(result: std::thread::Result<()>) {
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}
