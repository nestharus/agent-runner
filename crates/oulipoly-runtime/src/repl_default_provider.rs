use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_state::StateDb;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeServices {
    pub config_root: PathBuf,
    pub state_db_path: Option<PathBuf>,
    pub working_dir: Option<PathBuf>,
}

impl RuntimeServices {
    pub fn production(working_dir: Option<PathBuf>) -> Result<Self, String> {
        let config_root = dirs::config_dir()
            .map(|path| path.join("oulipoly-agent-runner"))
            .unwrap_or_else(|| PathBuf::from("oulipoly-agent-runner"));

        Ok(Self {
            config_root,
            state_db_path: None,
            working_dir,
        })
    }
}

#[allow(dead_code)]
pub(crate) trait InteractiveLauncher {
    fn launch(&self, provider: &ProviderConfig, working_dir: Option<&Path>) -> Result<i32, String>;
}

#[allow(dead_code)]
struct ProductionLauncher;

impl InteractiveLauncher for ProductionLauncher {
    fn launch(&self, provider: &ProviderConfig, working_dir: Option<&Path>) -> Result<i32, String> {
        crate::executor::cli::execute_interactive(provider, working_dir, None, None)
    }
}

pub fn run_repl_with_default_provider(services: RuntimeServices) -> Result<i32, String> {
    run_repl_with_default_provider_with_launcher(services, &ProductionLauncher)
}

#[allow(dead_code)]
pub(crate) fn run_repl_with_default_provider_with_launcher(
    services: RuntimeServices,
    launcher: &dyn InteractiveLauncher,
) -> Result<i32, String> {
    let config_path = services.config_root.join("config.toml");
    let app = oulipoly_config::app::AppConfig::load(&config_path)?;
    let family = app.default_provider.ok_or_else(|| {
        format!(
            "'default_provider' must be set in {} for 'agent' / '--new'",
            config_path.display()
        )
    })?;

    let providers_path = services.config_root.join("providers.toml");
    let providers = ProvidersConfig::load(&providers_path)?;
    let members = resolve_family_keys(&providers, &family);
    if members.is_empty() {
        return Err(format!(
            "default_provider '{family}' resolved to an empty provider pool in {}",
            providers_path.display()
        ));
    }

    let carrier_model = ModelConfig {
        name: format!("<provider-family:{family}>"),
        prompt_mode: PromptMode::Stdin,
        providers: members
            .iter()
            .map(|member| ProviderConfig::model_provider(*member, Vec::new()))
            .collect(),
        inputs: Vec::new(),
    };

    let state = match services.state_db_path.as_ref() {
        Some(path) => StateDb::open(path),
        None => StateDb::open_default(),
    }?;

    let idx = crate::balancer::select_provider(&carrier_model, &state, None);
    let member_name = members
        .get(idx)
        .ok_or_else(|| format!("selected provider index {idx} is out of bounds"))?;
    let (provider, _prompt_mode) = providers.runtime_provider(member_name)?;
    let launch_provider = ProviderConfig {
        name: carrier_model.name,
        ..provider
    };

    launcher.launch(&launch_provider, services.working_dir.as_deref())
}

#[allow(dead_code)]
pub(crate) fn resolve_family_keys<'a>(
    providers: &'a ProvidersConfig,
    family: &str,
) -> Vec<&'a str> {
    let mut exact = Vec::new();
    let mut suffixed = Vec::new();

    for key in providers.entries.keys() {
        let key = key.as_str();
        if key == family {
            exact.push(key);
            continue;
        }

        let Some(suffix) = key.strip_prefix(family) else {
            continue;
        };
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }

        suffixed.push((suffix, key));
    }

    exact.sort_unstable();
    suffixed.sort_by(|(left_suffix, left_key), (right_suffix, right_key)| {
        compare_digit_suffix(left_suffix, right_suffix).then_with(|| left_key.cmp(right_key))
    });

    exact
        .into_iter()
        .chain(suffixed.into_iter().map(|(_, key)| key))
        .filter(|key| match providers.runtime_provider(key) {
            Ok(_) => true,
            Err(error) => {
                tracing::warn!(
                    provider_name = *key,
                    error = error.as_str(),
                    "dropping invalid provider-family member"
                );
                false
            }
        })
        .collect()
}

fn compare_digit_suffix(left: &str, right: &str) -> std::cmp::Ordering {
    let left_trimmed = left.trim_start_matches('0');
    let right_trimmed = right.trim_start_matches('0');
    let left_numeric = if left_trimmed.is_empty() {
        "0"
    } else {
        left_trimmed
    };
    let right_numeric = if right_trimmed.is_empty() {
        "0"
    } else {
        right_trimmed
    };

    left_numeric
        .len()
        .cmp(&right_numeric.len())
        .then_with(|| left_numeric.cmp(right_numeric))
        .then_with(|| left.len().cmp(&right.len()))
        .then_with(|| left.cmp(right))
}

#[cfg(test)]
pub(crate) use resolve_family_keys as resolve_family_keys_for_test;

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::StateDb;
    use rusqlite::Connection;
    use std::cell::RefCell;

    fn runtime_services(config_root: PathBuf) -> RuntimeServices {
        RuntimeServices {
            config_root,
            state_db_path: None,
            working_dir: None,
        }
    }

    fn runtime_services_with_state(
        config_root: PathBuf,
        state_db_path: PathBuf,
    ) -> RuntimeServices {
        RuntimeServices {
            config_root,
            state_db_path: Some(state_db_path),
            working_dir: None,
        }
    }

    fn write_config(root: &Path, contents: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("config.toml"), contents).unwrap();
    }

    fn write_providers(root: &Path, contents: &str) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("providers.toml"), contents).unwrap();
    }

    fn provider_fixture(name: &str) -> String {
        format!(
            r#"[{name}]
command = "printf"
interactive_args = ["ok"]
"#
        )
    }

    fn load_providers(root: &Path, contents: &str) -> ProvidersConfig {
        write_providers(root, contents);
        ProvidersConfig::load(&root.join("providers.toml")).unwrap()
    }

    #[derive(Default)]
    struct RecordingLauncher {
        calls: RefCell<Vec<(String, Option<PathBuf>)>>,
    }

    impl InteractiveLauncher for RecordingLauncher {
        fn launch(
            &self,
            provider: &ProviderConfig,
            working_dir: Option<&Path>,
        ) -> Result<i32, String> {
            self.calls
                .borrow_mut()
                .push((provider.name.clone(), working_dir.map(Path::to_path_buf)));
            Ok(0)
        }
    }

    fn table_count(db_path: &Path, table: &str) -> i64 {
        let conn = Connection::open(db_path).unwrap();
        let sql = format!("SELECT COUNT(*) FROM {table}");
        conn.query_row(&sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn missing_default_provider_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        write_config(temp.path(), r#"diagnostics_model = "codex~high""#);

        let error = run_repl_with_default_provider(runtime_services(temp.path().to_path_buf()))
            .expect_err("missing default_provider should be rejected");

        assert_eq!(
            error,
            format!(
                "'default_provider' must be set in {} for 'agent' / '--new'",
                temp.path().join("config.toml").display()
            )
        );
    }

    #[test]
    fn empty_family_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("codex"));

        let error = run_repl_with_default_provider(runtime_services(temp.path().to_path_buf()))
            .expect_err("empty provider family should be rejected");

        assert_eq!(
            error,
            format!(
                "default_provider 'claude' resolved to an empty provider pool in {}",
                temp.path().join("providers.toml").display()
            )
        );
    }

    #[test]
    fn family_resolver_includes_exact_and_digit_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let providers = load_providers(
            temp.path(),
            &[
                provider_fixture("claude3"),
                provider_fixture("claude-work"),
                provider_fixture("claude"),
                provider_fixture("myclaude"),
                provider_fixture("claude2"),
            ]
            .join("\n"),
        );

        assert_eq!(
            resolve_family_keys_for_test(&providers, "claude"),
            vec!["claude", "claude2", "claude3"]
        );
    }

    #[test]
    fn family_resolver_excludes_dashed_and_prefixed_keys() {
        let temp = tempfile::tempdir().unwrap();
        let providers = load_providers(
            temp.path(),
            &[
                provider_fixture("claude-work"),
                provider_fixture("myclaude"),
                provider_fixture("claude2"),
                provider_fixture("codex"),
            ]
            .join("\n"),
        );

        assert_eq!(
            resolve_family_keys_for_test(&providers, "claude"),
            vec!["claude2"]
        );
    }

    #[test]
    fn family_resolver_drops_invalid_runtime_provider() {
        let temp = tempfile::tempdir().unwrap();
        let providers = load_providers(
            temp.path(),
            r#"[claude]
command = "printf"
interactive_args = ["ok"]

[claude2]
interactive_args = ["ok"]

[claude3]
command = "printf"
interactive_args = ["ok"]
"#,
        );

        assert_eq!(
            resolve_family_keys_for_test(&providers, "claude"),
            vec!["claude", "claude3"]
        );
    }

    #[test]
    fn synthetic_carrier_name_format() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path),
            &launcher,
        )
        .expect("synthetic carrier launch should succeed");

        assert_eq!(code, 0);
        assert_eq!(launcher.calls.borrow().len(), 1);
        assert_eq!(launcher.calls.borrow()[0].0, "<provider-family:claude>");
    }

    #[test]
    fn does_not_load_model_toml() {
        let temp = tempfile::tempdir().unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        std::fs::write(
            temp.path().join("claude.toml"),
            "this sentinel model TOML must not be read = [",
        )
        .unwrap();

        let launcher = RecordingLauncher::default();
        let code = run_repl_with_default_provider_with_launcher(
            runtime_services(temp.path().to_path_buf()),
            &launcher,
        )
        .expect("model TOML sentinels must not affect default-provider REPL launch");

        assert_eq!(code, 0);
        assert_eq!(launcher.calls.borrow().len(), 1);
    }

    #[test]
    fn does_not_create_invocation_row() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
            &launcher,
        )
        .expect("stubbed launcher should allow the default-provider path to complete");

        assert_eq!(code, 0);
        assert_eq!(table_count(&state_path, "invocations"), 0);
    }

    #[test]
    fn does_not_update_session_capture() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
            &launcher,
        )
        .expect("stubbed launcher should allow the default-provider path to complete");

        assert_eq!(code, 0);
        assert_eq!(table_count(&state_path, "invocations"), 0);
    }

    #[test]
    fn does_not_increment_quota_tick() {
        let temp = tempfile::tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        StateDb::open(&state_path).unwrap();
        write_config(temp.path(), r#"default_provider = "claude""#);
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services_with_state(temp.path().to_path_buf(), state_path.clone()),
            &launcher,
        )
        .expect("stubbed launcher should allow the default-provider path to complete");

        assert_eq!(code, 0);
        assert_eq!(table_count(&state_path, "provider_quotas"), 0);
        assert_eq!(table_count(&state_path, "provider_quota_windows"), 0);
    }

    #[test]
    fn does_not_call_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        write_config(
            temp.path(),
            r#"
diagnostics_model = "malformed-diagnostics-sentinel"
default_provider = "claude"
"#,
        );
        write_providers(temp.path(), &provider_fixture("claude"));
        let launcher = RecordingLauncher::default();

        let code = run_repl_with_default_provider_with_launcher(
            runtime_services(temp.path().to_path_buf()),
            &launcher,
        )
        .expect("diagnostics_model must not be consulted by the default-provider REPL");

        assert_eq!(code, 0);
        assert_eq!(launcher.calls.borrow().len(), 1);
    }
}
