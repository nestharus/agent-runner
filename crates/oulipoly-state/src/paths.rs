//! Agent-runner runtime path configuration.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const APP_DATA_DIR_NAME: &str = "oulipoly-agent-runner";
pub const DATA_DIR_ENV: &str = "OULIPOLY_DATA_DIR";
pub const CONFIG_HOME_ENV: &str = "OULIPOLY_CONFIG_HOME";
pub const ADJACENT_PATHS_FILE_NAME: &str = "config.toml";
const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimePathConfiguration {
    data_dir: PathBuf,
    config_home: PathBuf,
}

trait RuntimePathConfigurationProvider {
    fn data_dir(&self) -> Result<PathBuf, String>;
    fn config_home(&self) -> Result<PathBuf, String>;
}

struct AdjacentFileConfigurationProvider {
    path: PathBuf,
}

struct EnvironmentConfigurationProvider;

enum SelectedConfigurationProvider {
    AdjacentFile(AdjacentFileConfigurationProvider),
    Environment(EnvironmentConfigurationProvider),
}

impl RuntimePathConfigurationProvider for SelectedConfigurationProvider {
    fn data_dir(&self) -> Result<PathBuf, String> {
        match self {
            Self::AdjacentFile(provider) => provider.data_dir(),
            Self::Environment(provider) => provider.data_dir(),
        }
    }

    fn config_home(&self) -> Result<PathBuf, String> {
        match self {
            Self::AdjacentFile(provider) => provider.config_home(),
            Self::Environment(provider) => provider.config_home(),
        }
    }
}

struct RuntimePathConfigurationProviderFactory;

impl RuntimePathConfigurationProviderFactory {
    fn for_current_executable() -> Result<SelectedConfigurationProvider, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not resolve the current executable: {error}"))?;
        if is_deleted_linux_memfd(&executable) {
            return Ok(SelectedConfigurationProvider::Environment(
                EnvironmentConfigurationProvider,
            ));
        }
        Self::for_executable(&executable)
    }

    fn for_executable(executable: &Path) -> Result<SelectedConfigurationProvider, String> {
        let executable = std::fs::canonicalize(executable).map_err(|error| {
            format!(
                "Could not canonicalize executable {}: {error}",
                executable.display()
            )
        })?;
        let executable_dir = executable.parent().ok_or_else(|| {
            format!(
                "Could not resolve the directory containing executable {}",
                executable.display()
            )
        })?;
        let path = executable_dir.join(ADJACENT_PATHS_FILE_NAME);
        match path.try_exists() {
            Ok(true) => Ok(SelectedConfigurationProvider::AdjacentFile(
                AdjacentFileConfigurationProvider { path },
            )),
            Ok(false) => Ok(SelectedConfigurationProvider::Environment(
                EnvironmentConfigurationProvider,
            )),
            Err(error) => Err(format!(
                "Could not inspect runtime paths file {}: {error}",
                path.display()
            )),
        }
    }
}

#[cfg(target_os = "linux")]
fn is_deleted_linux_memfd(executable: &Path) -> bool {
    let Some(path) = executable.to_str() else {
        return false;
    };
    path.starts_with("/memfd:") && path.ends_with(" (deleted)") && !executable.exists()
}

#[cfg(not(target_os = "linux"))]
fn is_deleted_linux_memfd(_executable: &Path) -> bool {
    false
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdjacentPathConfigurationFile {
    data_dir: PathBuf,
    config_home: PathBuf,
}

impl RuntimePathConfigurationProvider for AdjacentFileConfigurationProvider {
    fn data_dir(&self) -> Result<PathBuf, String> {
        self.load().map(|configuration| configuration.data_dir)
    }

    fn config_home(&self) -> Result<PathBuf, String> {
        self.load().map(|configuration| configuration.config_home)
    }
}

impl AdjacentFileConfigurationProvider {
    fn load(&self) -> Result<RuntimePathConfiguration, String> {
        let text = std::fs::read_to_string(&self.path).map_err(|error| {
            format!(
                "Could not read runtime paths file {}: {error}",
                self.path.display()
            )
        })?;
        let configured: AdjacentPathConfigurationFile = toml::from_str(&text).map_err(|error| {
            format!(
                "Could not parse runtime paths file {}: {error}",
                self.path.display()
            )
        })?;
        require_absolute_file_path(&self.path, "data_dir", configured.data_dir).and_then(
            |data_dir| {
                require_absolute_file_path(&self.path, "config_home", configured.config_home).map(
                    |config_home| RuntimePathConfiguration {
                        data_dir,
                        config_home,
                    },
                )
            },
        )
    }
}

impl RuntimePathConfigurationProvider for EnvironmentConfigurationProvider {
    fn data_dir(&self) -> Result<PathBuf, String> {
        required_environment_path(DATA_DIR_ENV, "application data directory")
    }

    fn config_home(&self) -> Result<PathBuf, String> {
        std::env::var_os(CONFIG_HOME_ENV)
            .filter(|path| !path.is_empty())
            .or_else(|| std::env::var_os(XDG_CONFIG_HOME_ENV).filter(|path| !path.is_empty()))
            .ok_or_else(|| {
                format!(
                    "{CONFIG_HOME_ENV} is not set; set it to the runner's configuration home, for example: export {CONFIG_HOME_ENV}=/path/to/config-home"
                )
            })
            .map(PathBuf::from)
            .and_then(|path| absolutize_environment_path(CONFIG_HOME_ENV, path))
    }
}

pub fn data_dir() -> Result<PathBuf, String> {
    RuntimePathConfigurationProviderFactory::for_current_executable()?.data_dir()
}

pub fn config_dir() -> Result<PathBuf, String> {
    RuntimePathConfigurationProviderFactory::for_current_executable()?
        .config_home()
        .map(|config_home| config_home.join(APP_DATA_DIR_NAME))
}

fn required_environment_path(name: &str, purpose: &str) -> Result<PathBuf, String> {
    std::env::var_os(name)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| {
            format!(
                "{name} is not set; set it to the runner's {purpose}, for example: export {name}=/path/to/{}",
                if name == DATA_DIR_ENV {
                    "oulipoly-data"
                } else {
                    "config-home"
                }
            )
        })
        .map(PathBuf::from)
        .and_then(|path| absolutize_environment_path(name, path))
}

fn absolutize_environment_path(name: &str, path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir()
        .map(|current_dir| current_dir.join(path))
        .map_err(|error| format!("Could not resolve relative {name}: {error}"))
}

fn require_absolute_file_path(
    source: &Path,
    field: &str,
    path: PathBuf,
) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "Runtime paths file {} field {field} must be an absolute path",
            source.display()
        ));
    }
    Ok(path)
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
        old_oulipoly_config_home: Option<OsString>,
    }

    impl EnvGuard {
        fn set(oulipoly_data_dir: Option<&Path>, oulipoly_config_home: Option<&Path>) -> Self {
            let lock = env_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let old_oulipoly_data_dir = std::env::var_os(DATA_DIR_ENV);
            let old_oulipoly_config_home = std::env::var_os(CONFIG_HOME_ENV);
            unsafe {
                set_or_remove(DATA_DIR_ENV, oulipoly_data_dir);
                set_or_remove(CONFIG_HOME_ENV, oulipoly_config_home);
            }
            Self {
                _lock: lock,
                old_oulipoly_data_dir,
                old_oulipoly_config_home,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                restore_env(DATA_DIR_ENV, self.old_oulipoly_data_dir.take());
                restore_env(CONFIG_HOME_ENV, self.old_oulipoly_config_home.take());
            }
        }
    }

    #[test]
    fn environment_provider_requires_an_explicit_data_root() {
        let _guard = EnvGuard::set(None, None);

        assert!(EnvironmentConfigurationProvider.data_dir().is_err());
    }

    #[test]
    fn environment_provider_resolves_both_explicit_roots() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("isolated-data");
        let config_home = dir.path().join("isolated-config");
        let _guard = EnvGuard::set(Some(&data_root), Some(&config_home));

        assert_eq!(
            EnvironmentConfigurationProvider.data_dir().unwrap(),
            data_root
        );
        assert_eq!(
            EnvironmentConfigurationProvider.config_home().unwrap(),
            config_home
        );
    }

    #[test]
    fn factory_prefers_an_adjacent_file_over_environment() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("agents");
        std::fs::write(&executable, "fixture").unwrap();
        let file_data = dir.path().join("file-data");
        let file_config = dir.path().join("file-config");
        let env_data = dir.path().join("env-data");
        let env_config = dir.path().join("env-config");
        let _guard = EnvGuard::set(Some(&env_data), Some(&env_config));
        std::fs::write(
            dir.path().join(ADJACENT_PATHS_FILE_NAME),
            format!(
                "data_dir = {:?}\nconfig_home = {:?}\n",
                file_data.display().to_string(),
                file_config.display().to_string()
            ),
        )
        .unwrap();

        let selected =
            RuntimePathConfigurationProviderFactory::for_executable(&executable).unwrap();

        assert_eq!(selected.data_dir().unwrap(), file_data);
        assert_eq!(selected.config_home().unwrap(), file_config);
    }

    #[test]
    fn malformed_adjacent_file_does_not_fall_back_to_environment() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("agents");
        std::fs::write(&executable, "fixture").unwrap();
        let env_data = dir.path().join("env-data");
        let env_config = dir.path().join("env-config");
        let _guard = EnvGuard::set(Some(&env_data), Some(&env_config));
        std::fs::write(
            dir.path().join(ADJACENT_PATHS_FILE_NAME),
            "data_dir = [not-valid",
        )
        .unwrap();

        let error = RuntimePathConfigurationProviderFactory::for_executable(&executable)
            .unwrap()
            .data_dir()
            .unwrap_err();

        assert!(
            error.contains("Could not parse runtime paths file"),
            "{error}"
        );
    }

    #[test]
    fn factory_uses_environment_when_the_adjacent_file_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("agents");
        std::fs::write(&executable, "fixture").unwrap();
        let env_data = dir.path().join("env-data");
        let env_config = dir.path().join("env-config");
        let _guard = EnvGuard::set(Some(&env_data), Some(&env_config));

        let selected =
            RuntimePathConfigurationProviderFactory::for_executable(&executable).unwrap();

        assert_eq!(selected.data_dir().unwrap(), env_data);
        assert_eq!(selected.config_home().unwrap(), env_config);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn deleted_memfd_is_the_only_non_filesystem_executable_fallback() {
        assert!(is_deleted_linux_memfd(Path::new(
            "/memfd:agent-bash-delivery-helper (deleted)"
        )));
        assert!(!is_deleted_linux_memfd(Path::new(
            "/tmp/agent-bash-delivery-helper (deleted)"
        )));
        assert!(!is_deleted_linux_memfd(Path::new("/memfd:live-helper")));
    }

    #[test]
    fn adjacent_file_requires_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("agents");
        std::fs::write(&executable, "fixture").unwrap();
        std::fs::write(
            dir.path().join(ADJACENT_PATHS_FILE_NAME),
            "data_dir = \"relative-data\"\nconfig_home = \"/absolute-config\"\n",
        )
        .unwrap();

        let error = RuntimePathConfigurationProviderFactory::for_executable(&executable)
            .unwrap()
            .data_dir()
            .unwrap_err();

        assert!(
            error.contains("field data_dir must be an absolute path"),
            "{error}"
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
