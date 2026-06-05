#![cfg(target_os = "linux")]

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor::RuntimeExecutorService;
use oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest};
use oulipoly_state::pid_identity::PidIdentityDb;
use oulipoly_state::{CompositeInvocationId, StateDb};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

const INVOCATION_UUID: &str = "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa";

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_oulipoly_data_dir: Option<OsString>,
    old_xdg_data_home: Option<OsString>,
}

impl EnvGuard {
    fn set_xdg_data_home(path: &Path) -> Self {
        let lock = env_lock().lock().unwrap();
        let old_oulipoly_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::remove_var("OULIPOLY_DATA_DIR");
            std::env::set_var("XDG_DATA_HOME", path);
        }
        Self {
            _lock: lock,
            old_oulipoly_data_dir,
            old_xdg_data_home,
        }
    }

    fn set_xdg_data_home_and_oulipoly_data_dir(xdg_data_home: &Path, data_dir: &Path) -> Self {
        let lock = env_lock().lock().unwrap();
        let old_oulipoly_data_dir = std::env::var_os("OULIPOLY_DATA_DIR");
        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("OULIPOLY_DATA_DIR", data_dir);
            std::env::set_var("XDG_DATA_HOME", xdg_data_home);
        }
        Self {
            _lock: lock,
            old_oulipoly_data_dir,
            old_xdg_data_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match self.old_oulipoly_data_dir.take() {
                Some(value) => std::env::set_var("OULIPOLY_DATA_DIR", value),
                None => std::env::remove_var("OULIPOLY_DATA_DIR"),
            }
            match self.old_xdg_data_home.take() {
                Some(value) => std::env::set_var("XDG_DATA_HOME", value),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
    }
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn spawn_capture_writes_verified_sidecar_row_without_state_schema_change() {
    let dir = tempfile::tempdir().unwrap();
    let data_home = dir.path().join("data");
    let app_data_dir = data_home.join("oulipoly-agent-runner");
    let state_path = app_data_dir.join("state.db");
    let sidecar_path = app_data_dir.join("pid-identity.db");
    let (baseline_version, baseline_columns) = create_state_schema_snapshot(&state_path);
    let script = fixture_script(dir.path());
    let provider = fixture_provider(&script);
    let model = fixture_model(provider.clone());
    let invocation_env = invocation_env();

    {
        let _guard = EnvGuard::set_xdg_data_home(&data_home);
        let output = RuntimeExecutorService::default()
            .execute(ExecutorServiceRequest::Effective {
                model: model.clone(),
                provider: provider.clone(),
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "hello".to_string(),
                working_dir: None,
                extra_inputs: HashMap::new(),
                parent_invocation_env: Some(invocation_env),
            })
            .unwrap()
            .result;
        assert_eq!(output.exit_code, 0);
    }

    let sidecar = PidIdentityDb::open_read_only(&sidecar_path).unwrap();
    let rows = sidecar.lookup_by_invocation_uuid(INVOCATION_UUID).unwrap();
    assert_eq!(rows.len(), 1, "expected exactly one spawn sidecar row");
    let row = &rows[0];
    assert!(row.os_pid > 0);
    assert!(!row.os_boot_id.is_empty());
    assert!(row.os_pid_starttime_ticks > 0);
    assert_eq!(row.invocation_uuid, INVOCATION_UUID);
    assert_eq!(row.provider_name.as_deref(), Some("fixture-provider"));
    assert_eq!(row.model_name.as_deref(), Some("fixture-model"));

    let (after_version, after_columns) = read_state_schema_snapshot(&state_path);
    assert_eq!(after_version, baseline_version);
    assert_eq!(after_columns, baseline_columns);
}

#[test]
fn spawn_preserves_preexisting_oulipoly_data_dir_in_provider_child() {
    let dir = tempfile::tempdir().unwrap();
    let data_home = dir.path().join("data");
    let custom_data_dir = dir.path().join("custom-data-dir");
    std::fs::create_dir_all(&custom_data_dir).unwrap();
    let observed_env_path = dir.path().join("observed-oulipoly-data-dir.txt");
    let default_app_data_dir = data_home.join("oulipoly-agent-runner");
    let script = env_recording_fixture_script(dir.path());
    let mut provider = fixture_provider(&script);
    provider.args = vec![observed_env_path.to_string_lossy().into_owned()];
    let model = fixture_model(provider.clone());

    {
        let _guard =
            EnvGuard::set_xdg_data_home_and_oulipoly_data_dir(&data_home, &custom_data_dir);
        let output = RuntimeExecutorService::default()
            .execute(ExecutorServiceRequest::Effective {
                model: model.clone(),
                provider: provider.clone(),
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "hello".to_string(),
                working_dir: None,
                extra_inputs: HashMap::new(),
                parent_invocation_env: None,
            })
            .unwrap()
            .result;
        assert_eq!(output.exit_code, 0);
    }

    let observed = std::fs::read_to_string(&observed_env_path).unwrap();
    let expected_data_dir = custom_data_dir.to_string_lossy();
    assert_eq!(observed.trim_end(), expected_data_dir.as_ref());
    assert!(
        !default_app_data_dir.exists(),
        "provider child must not receive the runner's XDG-derived default data dir"
    );
}

fn create_state_schema_snapshot(path: &Path) -> (i64, Vec<String>) {
    let db = StateDb::open(path).unwrap();
    (
        user_version(db.connection()),
        invocation_columns(db.connection()),
    )
}

fn read_state_schema_snapshot(path: &Path) -> (i64, Vec<String>) {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    (user_version(&conn), invocation_columns(&conn))
}

fn user_version(conn: &Connection) -> i64 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn invocation_columns(conn: &Connection) -> Vec<String> {
    let mut stmt = conn.prepare("PRAGMA table_info(invocations)").unwrap();
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn fixture_script(dir: &Path) -> PathBuf {
    let path = dir.join("fixture-provider.sh");
    std::fs::write(
        &path,
        "#!/usr/bin/env bash\nset -euo pipefail\nsleep 0.2\nprintf 'ok\\n'\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn env_recording_fixture_script(dir: &Path) -> PathBuf {
    let path = dir.join("env-recording-provider.sh");
    std::fs::write(
        &path,
        "#!/usr/bin/env bash\nset -euo pipefail\nenv_path=\"${1:?missing env path}\"\nprintf '%s\\n' \"${OULIPOLY_DATA_DIR:?missing OULIPOLY_DATA_DIR}\" > \"$env_path\"\nprintf 'ok\\n'\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn fixture_provider(script: &Path) -> ProviderConfig {
    ProviderConfig {
        name: "fixture-provider".to_string(),
        command: script.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: Some(Vec::new()),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn fixture_model(provider: ProviderConfig) -> ModelConfig {
    ModelConfig {
        name: "fixture-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    }
}

fn invocation_env() -> String {
    serde_json::to_string(&CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: INVOCATION_UUID.to_string(),
    })
    .unwrap()
}
