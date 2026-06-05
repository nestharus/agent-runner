#![cfg(target_os = "linux")]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::PidIdentityDb;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

const DATA_DIR_ENV: &str = "OULIPOLY_DATA_DIR";

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_oulipoly_data_dir: Option<OsString>,
    old_xdg_data_home: Option<OsString>,
}

impl EnvGuard {
    fn set(oulipoly_data_dir: Option<&Path>, xdg_data_home: Option<&Path>) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_oulipoly_data_dir = std::env::var_os(DATA_DIR_ENV);
        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            set_or_remove(DATA_DIR_ENV, oulipoly_data_dir);
            set_or_remove("XDG_DATA_HOME", xdg_data_home);
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
            restore_env(DATA_DIR_ENV, self.old_oulipoly_data_dir.take());
            restore_env("XDG_DATA_HOME", self.old_xdg_data_home.take());
        }
    }
}

#[test]
fn default_state_locations_prefer_oulipoly_data_dir_over_xdg_data_home() {
    let dir = tempfile::tempdir().unwrap();
    let pinned = dir.path().join("canonical-app-data");
    let shadow_xdg = dir.path().join("shadow-xdg-data");
    let _guard = EnvGuard::set(Some(&pinned), Some(&shadow_xdg));

    assert_default_paths_under(&pinned);
}

#[test]
fn default_state_locations_fall_back_to_xdg_data_home_when_unpinned() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = dir.path().join("xdg-data");
    let _guard = EnvGuard::set(None, Some(&xdg));

    assert_default_paths_under(&xdg.join("oulipoly-agent-runner"));
}

fn assert_default_paths_under(app_data_dir: &Path) {
    assert_eq!(
        StateDb::default_path().unwrap(),
        app_data_dir.join("state.db")
    );
    assert_eq!(
        PidIdentityDb::default_path().unwrap(),
        app_data_dir.join("pid-identity.db")
    );
    assert_eq!(
        MailboxDb::default_path().unwrap(),
        PidIdentityDb::default_path().unwrap()
    );
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

unsafe fn set_or_remove(name: &str, value: Option<&Path>) {
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}

unsafe fn restore_env(name: &str, value: Option<OsString>) {
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
}
