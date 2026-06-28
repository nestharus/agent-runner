//! Default agent-runner data path resolution.

use std::path::{Path, PathBuf};

pub const APP_DATA_DIR_NAME: &str = "oulipoly-agent-runner";
pub const DATA_DIR_ENV: &str = "OULIPOLY_DATA_DIR";

pub fn data_dir() -> Result<PathBuf, String> {
    std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(default_data_dir)
}

fn default_data_dir() -> Result<PathBuf, String> {
    if running_under_test_harness() {
        return Err(
            "refusing to resolve the production data dir in a test or bench binary; set \
             OULIPOLY_DATA_DIR to an isolated temp dir"
                .to_string(),
        );
    }
    default_data_dir_unchecked()
}

fn default_data_dir_unchecked() -> Result<PathBuf, String> {
    let data_dir =
        dirs::data_dir().ok_or_else(|| "Could not determine data directory".to_string())?;
    Ok(data_dir.join(APP_DATA_DIR_NAME))
}

fn running_under_test_harness() -> bool {
    cfg!(test)
        || std::env::current_exe()
            .map(|path| path_has_deps_component(&path))
            .unwrap_or(false)
}

fn path_has_deps_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(name) if name == std::ffi::OsStr::new("deps")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
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
    fn deps_component_marks_cargo_test_binary_path() {
        let path = Path::new("/repo/target/debug/deps/foo-abc123");

        assert!(path_has_deps_component(path));
    }

    #[test]
    fn production_binary_paths_do_not_mark_test_binary() {
        for path in [
            Path::new("/home/nes/.local/bin/agents"),
            Path::new("/repo/target/release/agents"),
        ] {
            assert!(!path_has_deps_component(path));
        }
    }

    #[test]
    fn data_dir_refuses_production_default_under_test_without_override() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(None, Some(dir.path()));

        let error = data_dir().unwrap_err();

        assert!(
            error.contains("refusing to resolve the production data dir in a test or bench binary"),
            "unexpected error: {error}"
        );
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
    fn unchecked_default_keeps_xdg_fallback_shape_for_production() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = dir.path().join("xdg-data");
        let _guard = EnvGuard::set(None, Some(&xdg));

        assert_eq!(
            default_data_dir_unchecked().unwrap(),
            xdg.join(APP_DATA_DIR_NAME)
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
