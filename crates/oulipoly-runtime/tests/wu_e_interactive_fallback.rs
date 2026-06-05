#![cfg(unix)]

use oulipoly_config::{ProviderConfig, ResumeKind, ResumeStrategy};
use oulipoly_runtime::executor::cli::{
    ResumePayload, execute_interactive_with_result_and_model_identity,
};
use oulipoly_state::CompositeInvocationId;
use oulipoly_state::mailbox::MailboxDb;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

const INVOCATION_UUID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_xdg_data_home: Option<OsString>,
}

impl EnvGuard {
    fn set_xdg_data_home(path: &Path) -> Self {
        let lock = env_lock().lock().unwrap();
        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("XDG_DATA_HOME", path);
        }
        Self {
            _lock: lock,
            old_xdg_data_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
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
fn no_controlling_terminal_fallback_records_no_pty_control_path() {
    if std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
    {
        eprintln!("skipping fallback assertion because test process has a controlling terminal");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let data_home = dir.path().join("data");
    let sidecar_path = data_home
        .join("oulipoly-agent-runner")
        .join("pid-identity.db");
    let script = fixture_script(dir.path());
    let provider = fixture_provider(&script);
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };
    let invocation_env = invocation_env();

    {
        let _guard = EnvGuard::set_xdg_data_home(&data_home);
        let result = execute_interactive_with_result_and_model_identity(
            &provider,
            None,
            Some(&invocation_env),
            Some(ResumePayload {
                session_id: SESSION_ID,
                strategy: &strategy,
            }),
            Some("fixture-model"),
        )
        .unwrap();
        assert_eq!(result.exit_code, 0);
    }

    let db = MailboxDb::open(&sidecar_path).unwrap();
    let runtime = db.session_runtime(SESSION_ID).unwrap().unwrap();
    assert_eq!(runtime.mode, "pty_interactive");
    assert!(runtime.pty_control_path.is_none());
}

fn fixture_script(dir: &Path) -> PathBuf {
    let path = dir.join("fixture-provider.sh");
    fs::write(
        &path,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf 'ok\\n'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
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

fn invocation_env() -> String {
    serde_json::to_string(&CompositeInvocationId {
        source: "fixture".to_string(),
        id: INVOCATION_UUID.to_string(),
    })
    .unwrap()
}
