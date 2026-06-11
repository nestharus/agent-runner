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
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id,
        })
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations SET created_at = ?1 WHERE id = ?2",
            sqlite::params![created_at, id],
        )
        .unwrap();
    id
}

pub(in crate::db::tests) fn seed_running_invocation(db: &StateDb) -> i64 {
    db.start_invocation(&InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
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
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: model_name.to_string(),
            provider_name: provider_name.to_string(),
            provider_index,
            parent_invocation_id: None,
        })
        .unwrap();
    db.finalize_invocation(
        id,
        success,
        if success { 0 } else { 1 },
        error_category,
        stderr_snippet,
    )
    .unwrap();
    id
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
