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
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// A canonical runtime plan. The implementation must pin the named image,
/// cwd, recipe and input before K; paths and strings alone are not authority.
pub struct FreshProviderPlan {
    /// First token from the configured command. The resolved executable is
    /// separately pinned by the backend for this invocation.
    pub configured_program: String,
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
    if parts.is_empty() {
        return Err("fresh broker command has no first executable before K".into());
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
    let executable = resolve_first_executable(&parts[0], working_dir, &environment)?;
    let completion = backend.run_to_physical_q(FreshProviderPlan {
        configured_program: parts[0].clone(),
        executable,
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

/// Resolve in the assembled child's environment and cwd, once per attempt.
/// PATH candidates are probed in order with the caller's effective access;
/// the backend opens the chosen path and the broker pins that inode/mount.
fn resolve_first_executable(
    program: &str,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf, String> {
    if program.is_empty() || program.contains('\0') {
        return Err("fresh broker first executable is empty or contains NUL".into());
    }
    let path = Path::new(program);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    if program.contains('/') {
        return Ok(cwd.join(path));
    }
    // execvp's default when PATH is absent on Linux is the system path.
    let search = environment
        .get("PATH")
        .map(String::as_str)
        .unwrap_or("/bin:/usr/bin");
    for entry in search.split(':') {
        let directory = if entry.is_empty() {
            cwd.to_path_buf()
        } else {
            let entry = Path::new(entry);
            if entry.is_absolute() {
                entry.to_path_buf()
            } else {
                cwd.join(entry)
            }
        };
        let candidate = directory.join(program);
        if !candidate
            .metadata()
            .is_ok_and(|metadata| metadata.is_file())
        {
            continue;
        }
        let c_path = CString::new(candidate.as_os_str().as_bytes())
            .map_err(|_| "fresh broker PATH candidate contains NUL")?;
        if unsafe {
            libc::faccessat(
                libc::AT_FDCWD,
                c_path.as_ptr(),
                libc::X_OK,
                libc::AT_EACCESS,
            )
        } == 0
        {
            return Ok(candidate);
        }
    }
    Err(format!(
        "fresh broker command not found on child PATH before K: {program}"
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
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
    fn resume_shape_refuses_before_backend() {
        let mut backend = ObserveBackend {
            calls: 0,
            wait_status: 0,
        };
        let cwd = std::env::current_dir().unwrap();
        let mut resume = model("/bin/true");
        resume.prompt_mode = PromptMode::Arg;
        assert!(execute_fresh_headless(&resume, 0, "x", &cwd, &mut backend).is_err());
        assert_eq!(backend.calls, 0);
    }

    #[test]
    fn absolute_prefix_is_part_of_the_exact_broker_recipe() {
        let _lock = ENV_LOCK.lock().unwrap();
        let data = tempfile::tempdir().unwrap();
        let old_data = std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV);
        unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, data.path()) };
        struct PrefixBackend;
        impl FreshProviderBackend for PrefixBackend {
            fn run_to_physical_q(
                &mut self,
                plan: FreshProviderPlan,
            ) -> Result<FreshProviderCompletion, String> {
                assert_eq!(plan.executable, Path::new("/usr/bin/env"));
                assert_eq!(plan.configured_program, "/usr/bin/env");
                assert_eq!(plan.argv, ["-u", "CLAUDECODE", "/bin/true", "--fixture"]);
                Ok(FreshProviderCompletion {
                    wait_status: 0,
                    stdout: b"prefix output".to_vec(),
                    stderr: vec![],
                })
            }
        }
        let result = execute_fresh_headless(
            &model("/usr/bin/env -u CLAUDECODE /bin/true"),
            0,
            "raw prompt",
            &std::env::current_dir().unwrap(),
            &mut PrefixBackend,
        )
        .unwrap();
        assert_eq!(result.stdout, b"prefix output");
        match old_data {
            Some(value) => unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, value) },
            None => unsafe { std::env::remove_var(oulipoly_state::paths::DATA_DIR_ENV) },
        }
    }

    #[test]
    fn child_path_resolves_bare_command_and_preserves_prefix_and_configured_args() {
        let _lock = ENV_LOCK.lock().unwrap();
        let data = tempfile::tempdir().unwrap();
        let old_data = std::env::var_os(oulipoly_state::paths::DATA_DIR_ENV);
        unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, data.path()) };
        let bin = data.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let image = bin.join("env");
        fs::write(&image, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
        let mut configured = model("env -u CLAUDECODE claude");
        configured.providers[0]
            .environment
            .insert("PATH".into(), bin.display().to_string());
        struct CheckBackend {
            expected: PathBuf,
        }
        impl FreshProviderBackend for CheckBackend {
            fn run_to_physical_q(
                &mut self,
                plan: FreshProviderPlan,
            ) -> Result<FreshProviderCompletion, String> {
                assert_eq!(plan.configured_program, "env");
                assert_eq!(plan.executable, self.expected);
                assert_eq!(plan.argv, ["-u", "CLAUDECODE", "claude", "--fixture"]);
                assert_eq!(
                    plan.environment
                        .iter()
                        .find(|(key, _)| key == "PATH")
                        .unwrap()
                        .1,
                    self.expected.parent().unwrap().display().to_string()
                );
                Ok(FreshProviderCompletion {
                    wait_status: 0,
                    stdout: b"done".to_vec(),
                    stderr: vec![],
                })
            }
        }
        let result = execute_fresh_headless(
            &configured,
            0,
            "raw prompt",
            data.path(),
            &mut CheckBackend { expected: image },
        )
        .unwrap();
        assert_eq!(result.stdout, b"done");
        let mut absent = model("missing-provider");
        absent.providers[0]
            .environment
            .insert("PATH".into(), bin.display().to_string());
        let mut backend = ObserveBackend {
            calls: 0,
            wait_status: 0,
        };
        let error = execute_fresh_headless(&absent, 0, "raw prompt", data.path(), &mut backend)
            .err()
            .unwrap();
        assert!(error.contains("command not found"));
        assert_eq!(backend.calls, 0, "missing first command reached K backend");
        match old_data {
            Some(value) => unsafe { std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, value) },
            None => unsafe { std::env::remove_var(oulipoly_state::paths::DATA_DIR_ENV) },
        }
    }

    #[test]
    fn path_order_permissions_and_replacement_are_resolved_per_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        let first_image = first.join("provider");
        let second_image = second.join("provider");
        fs::write(&first_image, b"first").unwrap();
        fs::write(&second_image, b"second").unwrap();
        fs::set_permissions(&first_image, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&second_image, fs::Permissions::from_mode(0o755)).unwrap();
        let environment = BTreeMap::from([(
            "PATH".into(),
            format!("{}:{}", first.display(), second.display()),
        )]);
        assert_eq!(
            resolve_first_executable("provider", temp.path(), &environment).unwrap(),
            second_image
        );
        fs::set_permissions(&first_image, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolve_first_executable("provider", temp.path(), &environment).unwrap(),
            first_image
        );
        let pinned = fs::File::open(&first_image).unwrap();
        fs::rename(&first_image, first.join("old-provider")).unwrap();
        fs::write(&first_image, b"replacement").unwrap();
        fs::set_permissions(&first_image, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            resolve_first_executable("provider", temp.path(), &environment).unwrap(),
            first_image
        );
        assert_ne!(
            pinned.metadata().unwrap().ino(),
            first_image.metadata().unwrap().ino()
        );
        assert!(
            resolve_first_executable("missing", temp.path(), &environment)
                .unwrap_err()
                .contains("command not found")
        );
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
