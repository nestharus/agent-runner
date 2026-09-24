//! First private broker-direct headless backend. Launch policy and argument
//! assembly remain the normal runtime's; this branch never constructs a local
//! `Child` or uses the accepted-continuation sidecar identity.
use super::input_flags::resolve_input_flags;
use super::launch::{ProviderLaunchRequest, assemble_provider_launch};
use super::provider_lookup::provider_for_index;
use super::result::{execution_result_from_raw, raw_result_from_supervised_output};
use super::supervision::supervised_output_from_terminal;
use super::terminal_signal::terminal_status_from_exit_status;
use crate::executor::ExecutionResult;
use oulipoly_config::{InvocationMode, ModelConfig, PromptMode, ProvidersConfig, load_models};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// A canonical runtime plan. The implementation must pin the named image,
/// cwd, recipe and input before K; paths and strings alone are not authority.
pub struct FreshProviderPlan {
    pub executable: PathBuf,
    pub cwd: PathBuf,
    pub argv: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub stdin: Vec<u8>,
}

/// Constructed by a backend only after exact broker output and physical Q
/// readback. A provider exit or CLI return alone cannot satisfy this contract.
pub struct FreshProviderCompletion {
    pub wait_status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub trait FreshProviderBackend {
    fn run_to_physical_q(
        &mut self,
        plan: FreshProviderPlan,
    ) -> Result<FreshProviderCompletion, String>;
}

/// The closed fresh route may choose a real configured account when no quota
/// source is configured for that account. This makes no balance claim.
/// The v29 balancer is deliberately absent: it reads and mutates legacy State
/// and may spawn a quota refresh before the broker has issued K. A pool that
/// needs that decision must wait for a fresh-owned quota/selection authority.
#[derive(Debug)]
pub struct FreshConfiguredSelection {
    pub model: ModelConfig,
    pub provider_index: usize,
    pub provider_name: String,
}

pub fn load_configured_fresh_headless(
    config_dir: &Path,
    model_name: &str,
    provider_pin: Option<&str>,
) -> Result<FreshConfiguredSelection, String> {
    if !config_dir.is_absolute() {
        return Err("fresh config root is not absolute before K".into());
    }
    if model_name.is_empty()
        || model_name == "."
        || model_name == ".."
        || model_name.contains('/')
        || model_name.contains('\\')
        || model_name.starts_with('-')
    {
        return Err("fresh model name invalid before K".into());
    }
    let providers = ProvidersConfig::load(&config_dir.join("providers.toml"))
        .map_err(|e| format!("fresh providers config invalid before K: {e}"))?;
    let mut models = load_models(&config_dir.join("models"), Some(&providers))
        .map_err(|e| format!("fresh model config invalid before K: {e}"))?;
    let mut model = models
        .remove(model_name)
        .ok_or_else(|| format!("fresh model {model_name:?} absent before K"))?;
    let mut members = HashSet::new();
    for member in &model.providers {
        if !members.insert(member.name.as_str()) {
            return Err("fresh model has duplicate provider accounts before K".into());
        }
        if providers.get(&member.name).is_none() {
            return Err(format!("fresh provider {:?} absent before K", member.name));
        }
    }
    let provider_index = match provider_pin {
        Some(pin) if !pin.is_empty() => model
            .providers
            .iter()
            .position(|provider| provider.name == pin)
            .ok_or_else(|| format!("fresh provider pin {pin:?} absent before K"))?,
        Some(_) => return Err("fresh provider pin empty before K".into()),
        None if model.providers.len() == 1 => 0,
        None => {
            return Err(
                "fresh pool selection needs fresh-owned quota and routing evidence before K".into(),
            );
        }
    };
    let named = &model.providers[provider_index];
    let account = providers
        .get(&named.name)
        .ok_or_else(|| format!("fresh provider {:?} absent before K", named.name))?;
    if account.quota_script.is_some() || account.auth_refresh_command.is_some() {
        return Err("fresh provider quota authority unavailable before K".into());
    }
    let (effective, prompt_mode) = providers
        .effective_provider(named)
        .map_err(|e| format!("fresh provider config invalid before K: {e}"))?;
    let provider_name = effective.name.clone();
    model.prompt_mode = prompt_mode;
    model.providers[provider_index] = effective;
    Ok(FreshConfiguredSelection {
        model,
        provider_index,
        provider_name,
    })
}

/// Explicit narrow route. Unsupported shapes refuse before the backend sees
/// a plan, so no local supervisor or balancer can retry an uncertain K.
pub fn execute_fresh_headless(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: &Path,
    backend: &mut impl FreshProviderBackend,
) -> Result<ExecutionResult, String> {
    let provider = provider_for_index(model, provider_index)?;
    if model.prompt_mode != PromptMode::Stdin
        || !model.inputs.is_empty()
        || model.provider.is_some()
        || provider.invocation_mode != InvocationMode::Headless
        || provider.resume.is_some()
        || provider.session_capture.is_some()
        || provider.resume_acceptance.is_some()
        || provider.session_storage.is_some()
        || provider.system_prompt_override.is_some()
        || provider.tool_restrictions.is_some()
        || !working_dir.is_absolute()
    {
        return Err("fresh broker provider shape unsupported before K".into());
    }
    let parts = super::shell_split(&provider.command);
    if parts.len() != 1 || !Path::new(&parts[0]).is_absolute() {
        return Err("fresh broker requires one absolute executable before K".into());
    }
    let input_args = resolve_input_flags(model, &HashMap::new())?;
    if provider
        .environment
        .keys()
        .any(|key| forbidden_fresh_environment(key))
    {
        return Err("fresh broker provider environment unsupported before K".into());
    }
    let mut launch = assemble_provider_launch(
        ProviderLaunchRequest {
            provider,
            provider_args: &provider.args,
            tail_args: &[],
            prompt_mode: model.prompt_mode,
            prompt: Some(prompt),
            working_dir: Some(working_dir),
            input_args: &input_args,
            parent_invocation_env: None,
            start_known_provider_session_id: None,
        },
        None,
    )?;
    if launch.return_channel.is_some() || !launch.temp_files.is_empty() {
        return Err("fresh broker return channel or temp file unsupported before K".into());
    }
    // The private broker recipe excludes loader and broker control variables.
    // Make those removals part of the assembled Command before freezing its
    // effective environment so the broker executes exactly this plan.
    for key in std::env::vars_os().map(|(key, _)| key) {
        if key.to_str().is_some_and(forbidden_fresh_environment) {
            launch.cmd.env_remove(key);
        }
    }
    let cmd = &launch.cmd;
    if cmd.get_program() != parts[0].as_str() || cmd.get_current_dir() != Some(working_dir) {
        return Err("fresh broker launch command changed before K".into());
    }
    let mut environment = BTreeMap::<String, String>::new();
    for (key, value) in std::env::vars_os() {
        let key = key
            .to_str()
            .ok_or("fresh broker inherited environment key is not UTF-8")?;
        if forbidden_fresh_environment(key) {
            continue;
        }
        let value = value
            .to_str()
            .ok_or("fresh broker inherited environment value is not UTF-8")?;
        environment.insert(key.into(), value.into());
    }
    for (key, value) in cmd.get_envs() {
        let key = key
            .to_str()
            .ok_or("fresh broker environment key is not UTF-8")?;
        if let Some(value) = value {
            environment.insert(
                key.into(),
                value
                    .to_str()
                    .ok_or("fresh broker environment value is not UTF-8")?
                    .into(),
            );
        } else {
            environment.remove(key);
        }
    }
    if environment
        .keys()
        .any(|key| forbidden_fresh_environment(key))
    {
        return Err("fresh broker inherited environment unsupported before K".into());
    }
    let argv = cmd
        .get_args()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or("fresh broker argument is not UTF-8")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let stdin = launch
        .supervisor_config
        .prompt_payload
        .clone()
        .unwrap_or_default();
    let completion = backend.run_to_physical_q(FreshProviderPlan {
        executable: parts[0].clone().into(),
        cwd: working_dir.to_path_buf(),
        argv,
        environment: environment.into_iter().collect(),
        stdin,
    })?;
    let status = ExitStatus::from_raw(completion.wait_status);
    let terminal = supervised_output_from_terminal(
        &provider.name,
        launch.supervisor_config.recognizer,
        completion.stdout,
        completion.stderr,
        terminal_status_from_exit_status(&status),
        None,
        Some(status),
    );
    Ok(execution_result_from_raw(
        raw_result_from_supervised_output(&launch.capture_plan, terminal, Vec::new()),
        provider_index,
        None,
        None,
    ))
}

fn forbidden_fresh_environment(key: &str) -> bool {
    key.starts_with("LD_")
        || key.starts_with("DYLD_")
        || key.starts_with("OULIPOLY_KERNEL_")
        || matches!(key, "GLIBC_TUNABLES" | "GCONV_PATH")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::ProviderConfig;
    use std::fs;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct ObserveBackend {
        calls: usize,
        wait_status: i32,
    }
    impl FreshProviderBackend for ObserveBackend {
        fn run_to_physical_q(
            &mut self,
            plan: FreshProviderPlan,
        ) -> Result<FreshProviderCompletion, String> {
            self.calls += 1;
            assert_eq!(plan.argv, ["--fixture"]);
            assert_eq!(plan.stdin, b"raw prompt");
            Ok(FreshProviderCompletion {
                wait_status: self.wait_status,
                stdout: b"mapped output".to_vec(),
                stderr: b"diagnostic".to_vec(),
            })
        }
    }

    fn model(command: &str) -> ModelConfig {
        ModelConfig {
            name: "fixture".into(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::new(command, vec!["--fixture".into()])],
            inputs: Vec::new(),
            provider: None,
        }
    }

    #[test]
    fn real_config_selects_named_account_and_refuses_unowned_routing_inputs() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("models")).unwrap();
        fs::write(
            root.path().join("models/work.toml"),
            "[[providers]]\nname = 'other'\n[[providers]]\nname = 'chosen'\nargs = ['--model-option']\n",
        )
        .unwrap();
        fs::write(
            root.path().join("providers.toml"),
            "[other]\ncommand = '/bin/false'\n[chosen]\ncommand = '/bin/true'\nargs = ['--account-option']\n",
        )
        .unwrap();
        let selected = load_configured_fresh_headless(root.path(), "work", Some("chosen")).unwrap();
        assert_eq!(selected.provider_index, 1);
        assert_eq!(selected.provider_name, "chosen");
        assert_eq!(selected.model.providers[1].command, "/bin/true");
        assert_eq!(
            selected.model.providers[1].args,
            ["--account-option", "--model-option"]
        );
        assert!(
            load_configured_fresh_headless(root.path(), "work", None)
                .unwrap_err()
                .contains("fresh-owned quota and routing evidence")
        );
        assert!(
            load_configured_fresh_headless(root.path(), "work", Some("missing"))
                .unwrap_err()
                .contains("pin")
        );
        assert!(load_configured_fresh_headless(root.path(), "../work", Some("chosen")).is_err());
        fs::write(
            root.path().join("providers.toml"),
            "[other]\ncommand = '/bin/false'\n[chosen]\ncommand = '/bin/true'\nquota_script = 'must-not-run'\n",
        )
        .unwrap();
        assert!(
            load_configured_fresh_headless(root.path(), "work", Some("chosen"))
                .unwrap_err()
                .contains("quota authority unavailable")
        );
    }

    #[test]
    fn canonical_plan_maps_only_backend_q_completion() {
        let _lock = ENV_LOCK.lock().unwrap();
        let data = tempfile::tempdir().unwrap();
        let old_data = std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV);
        // The ordinary launch assembler requires an application data root.
        unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, data.path()) };
        let mut backend = ObserveBackend {
            calls: 0,
            wait_status: 0,
        };
        let result = execute_fresh_headless(
            &model("/bin/true"),
            0,
            "raw prompt",
            &std::env::current_dir().unwrap(),
            &mut backend,
        )
        .unwrap();
        assert_eq!(backend.calls, 1);
        assert_eq!(result.stdout, b"mapped output");
        assert_eq!(result.stderr, "diagnostic");
        assert_eq!(result.exit_code, 0);
        match old_data {
            Some(value) => unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, value) },
            None => unsafe { std::env::remove_var(oulipoly_state::paths::DATA_DIR_ENV) },
        }
    }

    #[test]
    fn prefixed_and_resume_shapes_refuse_before_backend() {
        let mut backend = ObserveBackend {
            calls: 0,
            wait_status: 0,
        };
        let cwd = std::env::current_dir().unwrap();
        assert!(
            execute_fresh_headless(&model("env /bin/true"), 0, "x", &cwd, &mut backend).is_err()
        );
        let mut resume = model("/bin/true");
        resume.prompt_mode = PromptMode::Arg;
        assert!(execute_fresh_headless(&resume, 0, "x", &cwd, &mut backend).is_err());
        assert_eq!(backend.calls, 0);
    }

    #[test]
    fn provider_failure_after_q_maps_as_failure() {
        let _lock = ENV_LOCK.lock().unwrap();
        let data = tempfile::tempdir().unwrap();
        let old_data = std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV);
        unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, data.path()) };
        let mut backend = ObserveBackend {
            calls: 0,
            wait_status: 7 << 8,
        };
        let result = execute_fresh_headless(
            &model("/bin/true"),
            0,
            "raw prompt",
            &std::env::current_dir().unwrap(),
            &mut backend,
        )
        .unwrap();
        assert_eq!(result.exit_code, 7);
        assert_eq!(result.terminal_reason.as_deref(), Some("exit_nonzero"));
        match old_data {
            Some(value) => unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, value) },
            None => unsafe { std::env::remove_var(oulipoly_state::paths::DATA_DIR_ENV) },
        }
    }
}
