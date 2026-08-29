//! Agent-runner data path resolution.

use std::path::PathBuf;

pub const APP_DATA_DIR_NAME: &str = "oulipoly-agent-runner";
pub const DATA_DIR_ENV: &str = "OULIPOLY_DATA_DIR";
pub const CONFIG_HOME_ENV: &str = "OULIPOLY_CONFIG_HOME";
const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";

pub fn data_dir() -> Result<PathBuf, String> {
    std::env::var_os(DATA_DIR_ENV)
        .ok_or_else(|| {
            format!(
                "{DATA_DIR_ENV} is not set; set it to the runner's application data directory, for example: export {DATA_DIR_ENV}=/path/to/oulipoly-data"
            )
        })
        .map(PathBuf::from)
        .and_then(absolutize_configured_data_dir)
}

pub fn config_dir() -> Result<PathBuf, String> {
    std::env::var_os(CONFIG_HOME_ENV)
        .or_else(|| std::env::var_os(XDG_CONFIG_HOME_ENV))
        .ok_or_else(|| {
            format!(
                "{CONFIG_HOME_ENV} is not set; set it to the runner's configuration home, for example: export {CONFIG_HOME_ENV}=/path/to/config-home"
            )
        })
        .map(PathBuf::from)
        .and_then(absolutize_configured_config_home)
        .map(|home| home.join(APP_DATA_DIR_NAME))
}

fn absolutize_configured_data_dir(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .map_err(|error| format!("Could not resolve relative {DATA_DIR_ENV}: {error}"))
}

fn absolutize_configured_config_home(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .map_err(|error| format!("Could not resolve relative {CONFIG_HOME_ENV}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;
    use std::sync::{Mutex, MutexGuard, OnceLock};

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
    fn data_dir_requires_explicit_environment_even_when_xdg_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(None, Some(dir.path()));

        let error = data_dir().unwrap_err();

        assert!(error.contains("OULIPOLY_DATA_DIR is not set"), "{error}");
        assert!(error.contains("export OULIPOLY_DATA_DIR="), "{error}");
    }

    #[test]
    fn data_dir_override_bypasses_test_guard_and_default_open_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("isolated-data");
        let _guard = EnvGuard::set(Some(&data_root), None);

        assert_eq!(data_dir().unwrap(), data_root);
        let _db = crate::StateDb::open_default().unwrap();
        assert!(data_root.join("state.db").exists());
    }

    #[test]
    fn relative_data_dir_override_is_pinned_to_one_absolute_source_path() {
        let relative = Path::new("target/age299-relative-data");
        let _guard = EnvGuard::set(Some(relative), None);

        assert_eq!(
            data_dir().unwrap(),
            std::env::current_dir().unwrap().join(relative)
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
}
