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
use oulipoly_config::{InvocationMode, ModelConfig, PromptMode};
use std::collections::{BTreeMap, HashMap};
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
