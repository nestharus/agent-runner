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
use crate::executor::terminal_signal::TerminalSignal;
use oulipoly_config::{InvocationMode, ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// A canonical runtime plan. The implementation must pin the named image,
/// cwd, recipe and input before K; paths and strings alone are not authority.
#[derive(Clone)]
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

/// The same recognizer identity used by the normal CLI supervisor, frozen
/// with a broker-verified route candidate before provider K.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FreshTerminalRecognizer {
    Codex,
    OpenCode,
    OpenAiCompat,
}

impl FreshTerminalRecognizer {
    pub fn for_provider(provider: &ProviderConfig) -> Self {
        match super::provider_identity::ProviderRecognizer::for_provider(provider) {
            super::provider_identity::ProviderRecognizer::Codex => Self::Codex,
            super::provider_identity::ProviderRecognizer::OpenCode => Self::OpenCode,
            super::provider_identity::ProviderRecognizer::OpenAiCompat => Self::OpenAiCompat,
        }
    }

    /// Classify the original provider bytes and wait status. Consumers must
    /// obtain these from the exact physical Q, never a CLI return code alone.
    pub fn classify(
        self,
        provider_name: &str,
        stdout: &[u8],
        stderr: &[u8],
        wait_status: i32,
    ) -> TerminalSignal {
        let recognizer = match self {
            Self::Codex => super::provider_identity::ProviderRecognizer::Codex,
            Self::OpenCode => super::provider_identity::ProviderRecognizer::OpenCode,
            Self::OpenAiCompat => super::provider_identity::ProviderRecognizer::OpenAiCompat,
        };
        super::terminal_signal::recognize_terminal_signal(
            provider_name,
            recognizer,
            stdout,
            stderr,
            terminal_status_from_exit_status(&ExitStatus::from_raw(wait_status)),
        )
    }
}

pub trait FreshProviderBackend {
    fn run_to_physical_q(
        &mut self,
        plan: FreshProviderPlan,
    ) -> Result<FreshProviderCompletion, String>;
}

pub struct PreparedFreshHeadless {
    pub plan: FreshProviderPlan,
    launch: super::launch::ProviderLaunch,
    provider_name: String,
    provider_index: usize,
}

#[derive(Debug)]
pub struct FreshConfiguredPool {
    pub model: ModelConfig,
    pub config_sha256: String,
    pub account_effects: Vec<(Option<String>, Option<String>)>,
    pub account_identities: Vec<Option<String>>,
}

/// The two source files are identified as bytes before any provider plan is
/// offered to the broker. The broker binds its durable choice to this identity
/// and to the sealed candidate plans; a later edit cannot change an uncertain K.
pub fn load_fresh_headless_pool(
    config_dir: &Path,
    model_name: &str,
) -> Result<FreshConfiguredPool, String> {
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
    let provider_path = config_dir.join("providers.toml");
    let model_path = config_dir.join("models").join(format!("{model_name}.toml"));
    let provider_bytes = std::fs::read(&provider_path)
        .map_err(|e| format!("fresh providers source unavailable before K: {e}"))?;
    let model_bytes = std::fs::read(&model_path)
        .map_err(|e| format!("fresh model source unavailable before K: {e}"))?;
    let mut hash = Sha256::new();
    for bytes in [&provider_bytes, &model_bytes] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let provider_text = std::str::from_utf8(&provider_bytes)
        .map_err(|e| format!("fresh providers source is not UTF-8 before K: {e}"))?;
    let model_text = std::str::from_utf8(&model_bytes)
        .map_err(|e| format!("fresh model source is not UTF-8 before K: {e}"))?;
    let providers = ProvidersConfig::from_toml(provider_text)
        .map_err(|e| format!("fresh providers config invalid before K: {e}"))?;
    let mut model = ModelConfig::from_toml_with_name(model_name, model_text, Some(&providers))
        .map_err(|e| format!("fresh model config invalid before K: {e}"))?;
    let mut members = HashSet::new();
    let mut prompt_mode = None;
    let mut account_effects = Vec::new();
    let mut account_identities = Vec::new();
    for member in &mut model.providers {
        if !members.insert(member.name.clone()) {
            return Err("fresh model has duplicate provider accounts before K".into());
        }
        let account = providers
            .get(&member.name)
            .ok_or_else(|| format!("fresh provider {:?} absent before K", member.name))?;
        let effect = crate::quota::fresh_refresh_source(&member.name, &providers)
            .map(|(quota, auth)| (Some(quota), auth))
            .unwrap_or((None, account.auth_refresh_command.clone()));
        account_effects.push(effect);
        account_identities.push(account.quota_account_id.clone());
        let (effective, mode) = providers
            .effective_provider(member)
            .map_err(|e| format!("fresh provider config invalid before K: {e}"))?;
        if prompt_mode.is_some_and(|previous| previous != mode) {
            return Err("fresh pool has incompatible prompt modes before K".into());
        }
        prompt_mode = Some(mode);
        *member = effective;
    }
    if model.providers.is_empty() {
        return Err("fresh model has no accounts before K".into());
    }
    model.prompt_mode = prompt_mode.ok_or("fresh model has no prompt mode before K")?;
    let after_provider = std::fs::read(&provider_path).map_err(|e| e.to_string())?;
    let after_model = std::fs::read(&model_path).map_err(|e| e.to_string())?;
    if after_provider != provider_bytes || after_model != model_bytes {
        return Err("fresh config changed during load before K".into());
    }
    Ok(FreshConfiguredPool {
        model,
        config_sha256: format!("{:x}", hash.finalize()),
        account_effects,
        account_identities,
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
    let prepared = prepare_fresh_headless(model, provider_index, prompt, working_dir)?;
    run_prepared_fresh_headless(prepared, backend)
}

/// Structural preflight shared by the original Runner plan and the broker's
/// source-owned manual quota probe. It does not read inherited process env.
pub fn validate_fresh_headless_shape(
    model: &ModelConfig,
    provider_index: usize,
    working_dir: &Path,
) -> Result<(), String> {
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
    if provider
        .environment
        .keys()
        .any(|key| forbidden_fresh_environment(key))
    {
        return Err("fresh broker provider environment unsupported before K".into());
    }
    Ok(())
}

pub fn prepare_fresh_headless(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: &Path,
) -> Result<PreparedFreshHeadless, String> {
    validate_fresh_headless_shape(model, provider_index, working_dir)?;
    let provider = provider_for_index(model, provider_index)?;
    let parts = super::shell_split(&provider.command);
    let input_args = resolve_input_flags(model, &HashMap::new())?;
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
    Ok(PreparedFreshHeadless {
        plan: FreshProviderPlan {
            executable: parts[0].clone().into(),
            cwd: working_dir.to_path_buf(),
            argv,
            environment: environment.into_iter().collect(),
            stdin,
        },
        launch,
        provider_name: provider.name.clone(),
        provider_index,
    })
}

pub fn run_prepared_fresh_headless(
    prepared: PreparedFreshHeadless,
    backend: &mut impl FreshProviderBackend,
) -> Result<ExecutionResult, String> {
    let PreparedFreshHeadless {
        plan,
        launch,
        provider_name,
        provider_index,
    } = prepared;
    let completion = backend.run_to_physical_q(plan)?;
    let status = ExitStatus::from_raw(completion.wait_status);
    let terminal = supervised_output_from_terminal(
        &provider_name,
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
    fn real_config_prepares_two_accounts_and_records_quota_source() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("models")).unwrap();
        fs::write(
            root.path().join("models/work.toml"),
            "[[providers]]\nname = 'other'\n[[providers]]\nname = 'chosen'\nargs = ['--model-option']\n",
        )
        .unwrap();
        fs::write(
            root.path().join("providers.toml"),
            "[other]\ncommand = '/bin/false'\nquota_account_id = 'physical-other'\n[chosen]\ncommand = '/bin/true'\nquota_account_id = 'physical-chosen'\nargs = ['--account-option']\n",
        )
        .unwrap();
        let pool = load_fresh_headless_pool(root.path(), "work").unwrap();
        assert_eq!(pool.model.providers.len(), 2);
        assert_eq!(pool.model.providers[1].name, "chosen");
        assert_eq!(pool.model.providers[1].command, "/bin/true");
        assert_eq!(pool.config_sha256.len(), 64);
        assert_eq!(
            pool.account_identities[1].as_deref(),
            Some("physical-chosen")
        );
        assert_eq!(
            pool.model.providers[1].args,
            ["--account-option", "--model-option"]
        );
        fs::write(
            root.path().join("providers.toml"),
            "[other]\ncommand = '/bin/true'\n[chosen]\ncommand = '/bin/true'\nargs = ['--account-option']\n",
        )
        .unwrap();
        assert_ne!(
            load_fresh_headless_pool(root.path(), "work")
                .unwrap()
                .config_sha256,
            pool.config_sha256
        );
        assert!(load_fresh_headless_pool(root.path(), "../work").is_err());
        fs::write(
            root.path().join("providers.toml"),
            "[other]\ncommand = '/bin/false'\n[chosen]\ncommand = '/bin/true'\nquota_script = 'must-not-run'\n",
        )
        .unwrap();
        let metered = load_fresh_headless_pool(root.path(), "work").unwrap();
        assert_eq!(
            metered.account_effects[1].0.as_deref(),
            Some("must-not-run")
        );
        assert!(metered.account_identities[1].is_none());
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

    #[test]
    fn uncertain_backend_never_maps_a_provider_result_or_replays() {
        struct UnknownBackend(usize);
        impl FreshProviderBackend for UnknownBackend {
            fn run_to_physical_q(
                &mut self,
                _plan: FreshProviderPlan,
            ) -> Result<FreshProviderCompletion, String> {
                self.0 += 1;
                Err("fresh provider unknown: exact attempt".into())
            }
        }
        let _lock = ENV_LOCK.lock().unwrap();
        let data = tempfile::tempdir().unwrap();
        let old_data = std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV);
        unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, data.path()) };
        let mut backend = UnknownBackend(0);
        let result = execute_fresh_headless(
            &model("/bin/true"),
            0,
            "raw prompt",
            &std::env::current_dir().unwrap(),
            &mut backend,
        );
        assert_eq!(result.unwrap_err(), "fresh provider unknown: exact attempt");
        assert_eq!(backend.0, 1);
        match old_data {
            Some(value) => unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, value) },
            None => unsafe { std::env::remove_var(oulipoly_state::paths::DATA_DIR_ENV) },
        }
    }
}
