#![cfg(unix)]

use oulipoly_runtime::repl_default_provider::{
    RuntimeServices, run_repl_with_default_provider,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvRestore {
    old_data_home: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn set_xdg_data_home(path: &Path) -> Self {
        let old_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            std::env::set_var("XDG_DATA_HOME", path);
        }
        Self { old_data_home }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        match self.old_data_home.take() {
            Some(value) => unsafe {
                std::env::set_var("XDG_DATA_HOME", value);
            },
            None => unsafe {
                std::env::remove_var("XDG_DATA_HOME");
            },
        }
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn age_33_runtime_default_provider_uses_explicit_state_db_path_when_supplied() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("config-root");
    fs::create_dir_all(&config_root).unwrap();
    let explicit_state_db = temp.path().join("explicit-state").join("state.db");
    let blocked_default_data_home = temp.path().join("blocked-default-data-home");
    fs::write(&blocked_default_data_home, "not a directory").unwrap();
    let _restore = EnvRestore::set_xdg_data_home(&blocked_default_data_home);

    let marker = temp.path().join("launched.txt");
    let provider_script = temp.path().join("provider.sh");
    write_executable(
        &provider_script,
        &format!(
            "printf '%s' \"${{1:-missing}}\" > {:?}\n",
            marker.to_string_lossy()
        ),
    );
    fs::write(
        config_root.join("config.toml"),
        r#"default_provider = "fixture""#,
    )
    .unwrap();
    fs::write(
        config_root.join("providers.toml"),
        format!(
            r#"[fixture]
command = {:?}
args = ["one-shot-only"]
interactive_args = ["interactive-launch"]
prompt_mode = "arg"
"#,
            provider_script.to_string_lossy()
        ),
    )
    .unwrap();

    let status = run_repl_with_default_provider(RuntimeServices {
        config_root,
        state_db_path: Some(PathBuf::from(&explicit_state_db)),
        working_dir: None,
    })
    .unwrap();

    assert_eq!(status, 0);
    assert_eq!(fs::read_to_string(&marker).unwrap(), "interactive-launch");
    assert!(explicit_state_db.exists());
    assert!(
        !blocked_default_data_home.join("oulipoly-agent-runner").exists(),
        "explicit state_db_path should avoid StateDb::open_default path discovery"
    );
}

