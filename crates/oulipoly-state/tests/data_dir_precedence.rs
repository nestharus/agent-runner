#![cfg(target_os = "linux")]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::PidIdentityDb;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

const DATA_DIR_ENV: &str = "OULIPOLY_DATA_DIR";
const CONFIG_HOME_ENV: &str = "OULIPOLY_CONFIG_HOME";

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    old_oulipoly_data_dir: Option<OsString>,
    old_oulipoly_config_home: Option<OsString>,
    old_xdg_data_home: Option<OsString>,
}

impl EnvGuard {
    fn set(
        oulipoly_data_dir: Option<&Path>,
        oulipoly_config_home: Option<&Path>,
        xdg_data_home: Option<&Path>,
    ) -> Self {
        let lock = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_oulipoly_data_dir = std::env::var_os(DATA_DIR_ENV);
        let old_oulipoly_config_home = std::env::var_os(CONFIG_HOME_ENV);
        let old_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
        unsafe {
            set_or_remove(DATA_DIR_ENV, oulipoly_data_dir);
            set_or_remove(CONFIG_HOME_ENV, oulipoly_config_home);
            set_or_remove("XDG_DATA_HOME", xdg_data_home);
        }
        Self {
            _lock: lock,
            old_oulipoly_data_dir,
            old_oulipoly_config_home,
            old_xdg_data_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            restore_env(DATA_DIR_ENV, self.old_oulipoly_data_dir.take());
            restore_env(CONFIG_HOME_ENV, self.old_oulipoly_config_home.take());
            restore_env("XDG_DATA_HOME", self.old_xdg_data_home.take());
        }
    }
}

#[test]
fn default_state_locations_use_required_oulipoly_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let pinned = dir.path().join("canonical-app-data");
    let config_home = dir.path().join("config-home");
    let shadow_xdg = dir.path().join("shadow-xdg-data");
    let _guard = EnvGuard::set(Some(&pinned), Some(&config_home), Some(&shadow_xdg));

    assert_default_paths_under(&pinned);
}

#[test]
fn default_state_locations_refuse_xdg_data_home_when_unpinned() {
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config-home");
    let xdg = dir.path().join("xdg-data");
    let _guard = EnvGuard::set(None, Some(&config_home), Some(&xdg));

    assert_default_paths_refuse_unpinned();
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

fn assert_default_paths_refuse_unpinned() {
    assert!(StateDb::default_path().is_err());
    assert!(PidIdentityDb::default_path().is_err());
    assert!(MailboxDb::default_path().is_err());
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
