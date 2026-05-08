pub mod cli;

pub use oulipoly_agent_messenger::ReturnedArtifactRef;
use oulipoly_config::ModelConfig;
use oulipoly_state::CompositeInvocationId;
use std::collections::HashMap;
use std::path::Path;

#[allow(dead_code)]
pub struct ExecutionResult {
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// Numeric child-process exit code per `exit_code_from_status`.
    pub exit_code: i32,
    pub provider_index: usize,
    pub session_capture: SessionCaptureResult,
    pub resume_acceptance: Option<ResumeAcceptanceResult>,
    pub terminal_reason: Option<String>,
    pub captured_child_invocations: Vec<CapturedChildInvocation>,
    pub returned_artifacts: Vec<ReturnedArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedChildInvocation {
    pub composite_id: CompositeInvocationId,
    pub raw_marker_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeAcceptanceResult {
    pub status: ResumeAcceptanceStatus,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAcceptanceStatus {
    Accepted,
    Rejected,
    Unconfirmed,
}

impl ResumeAcceptanceStatus {
    pub fn db_value(self) -> &'static str {
        match self {
            ResumeAcceptanceStatus::Accepted => "accepted",
            ResumeAcceptanceStatus::Rejected => "rejected",
            ResumeAcceptanceStatus::Unconfirmed => "unconfirmed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionCaptureResult {
    pub session_id: Option<String>,
    pub method: SessionCaptureMethod,
}

#[derive(Debug, Clone)]
pub enum SessionCaptureMethod {
    None,
    ForcedFlagVerified,
    StdoutJsonEvent,
    Failed(String),
}

impl SessionCaptureMethod {
    pub fn db_value(&self) -> &'static str {
        match self {
            SessionCaptureMethod::None => "none",
            SessionCaptureMethod::ForcedFlagVerified => "forced_flag_verified",
            SessionCaptureMethod::StdoutJsonEvent => "stdout_json_event",
            SessionCaptureMethod::Failed(_) => "failed",
        }
    }
}

pub use cli::provider_name;

/// Execute a model with the original prompt-only interface (backwards compat).
pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
) -> Result<ExecutionResult, String> {
    cli::execute(
        model,
        provider_index,
        prompt,
        working_dir,
        &HashMap::new(),
        None,
    )
}

/// Execute a model with extra inputs mapped to CLI flags.
pub fn execute_with_inputs(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<ExecutionResult, String> {
    cli::execute(
        model,
        provider_index,
        prompt,
        working_dir,
        extra_inputs,
        None,
    )
}

/// Execute a model with extra inputs and an explicit parent-invocation env payload.
pub fn execute_with_inputs_and_env(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    parent_invocation_env: Option<&str>,
) -> Result<ExecutionResult, String> {
    cli::execute(
        model,
        provider_index,
        prompt,
        working_dir,
        extra_inputs,
        parent_invocation_env,
    )
}

pub fn execute_effective_with_inputs_and_env(
    request: cli::EffectiveExecuteRequest<'_>,
) -> Result<ExecutionResult, String> {
    cli::execute_effective(request)
}

// Characterization test for AGE-8 — pins current behavior of executor/mod.rs facade wrappers in this inline test module.
#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::{InputDef, InputType, PromptMode, ProviderConfig};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    struct FixtureScript {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    #[cfg(unix)]
    fn fixture_script(body: &str) -> FixtureScript {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("executor-facade-fixture.sh");
        std::fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        FixtureScript { _dir: dir, path }
    }

    #[cfg(unix)]
    fn arg_model(script: &FixtureScript) -> ModelConfig {
        ModelConfig {
            name: "facade-fixture".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new(
                script.path.to_string_lossy().into_owned(),
                vec![],
            )],
            inputs: vec![],
        }
    }

    // Characterization test for AGE-8 — pins current behavior of executor/mod.rs execute facade wrapper.
    // In-bounds delegation: the wrapper passes provider_index through to cli::execute and the supplied
    // provider runs with the given prompt. (Out-of-bounds provider_index returns Err per cli::execute;
    // characterized by execute_wrapper_returns_err_when_provider_index_out_of_range below.)
    #[cfg(unix)]
    #[test]
    fn execute_wrapper_delegates_prompt_and_provider_index_to_cli_executor() {
        let script = fixture_script("printf 'argv=%s\\n' \"$1\"");
        let result = execute(&arg_model(&script), 0, "hello facade", None).unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.provider_index, 0);
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "argv=hello facade\n"
        );
    }

    // Characterization test for AGE-8 — pins current Err behavior when provider_index is out of bounds.
    #[cfg(unix)]
    #[test]
    fn execute_wrapper_returns_err_when_provider_index_out_of_range() {
        let script = fixture_script("printf 'unused\\n'");
        match execute(&arg_model(&script), 3, "ignored", None) {
            Ok(_) => panic!("expected Err for out-of-range provider_index"),
            Err(err) => assert!(
                err.contains("Provider index 3 out of range"),
                "unexpected err: {err}"
            ),
        }
    }

    // Characterization test for AGE-8 — pins current behavior of executor/mod.rs execute_with_inputs facade wrapper.
    #[cfg(unix)]
    #[test]
    fn execute_with_inputs_wrapper_delegates_schema_mapped_flags() {
        let script = fixture_script("printf '%s\\n' \"$@\"");
        let mut model = arg_model(&script);
        model.inputs.push(InputDef {
            name: "size".to_string(),
            input_type: InputType::String,
            required: false,
            default_input: false,
            default: None,
            description: None,
            flag: Some("--size".to_string()),
        });
        let mut inputs = HashMap::new();
        inputs.insert("size".to_string(), vec!["large".to_string()]);

        let result = execute_with_inputs(&model, 0, "draw", None, &inputs).unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "--size\nlarge\ndraw\n"
        );
    }

    // Characterization test for AGE-8 — pins current behavior of executor/mod.rs execute_with_inputs_and_env facade wrapper.
    #[cfg(unix)]
    #[test]
    fn execute_with_inputs_and_env_wrapper_delegates_parent_invocation_env() {
        let env_dump = tempfile::NamedTempFile::new().unwrap();
        let env_dump_path = env_dump.path().to_path_buf();
        let script = fixture_script(&format!(
            r#"printf '%s' "${{OULIPOLY_PARENT_INVOCATION-}}" > "{dump}"
printf 'ok\n'"#,
            dump = env_dump_path.display()
        ));
        let parent_env =
            r#"{"source":"fixture-provider","id":"11111111-1111-1111-1111-111111111111"}"#;

        let result = execute_with_inputs_and_env(
            &arg_model(&script),
            0,
            "prompt",
            None,
            &HashMap::new(),
            Some(parent_env),
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(std::fs::read_to_string(env_dump_path).unwrap(), parent_env);
    }

    // Characterization test for AGE-8 — pins current behavior of executor/mod.rs execute_effective_with_inputs_and_env facade wrapper.
    #[cfg(unix)]
    #[test]
    fn execute_effective_with_inputs_and_env_wrapper_uses_supplied_effective_provider() {
        let model_script = fixture_script("printf 'model-provider\\n'");
        let effective_script = fixture_script("printf 'effective=%s\\n' \"$1\"");
        let model = arg_model(&model_script);
        let effective_provider =
            ProviderConfig::new(effective_script.path.to_string_lossy().into_owned(), vec![]);

        let result = execute_effective_with_inputs_and_env(cli::EffectiveExecuteRequest {
            model: &model,
            provider: &effective_provider,
            provider_index: 9,
            prompt_mode: PromptMode::Arg,
            prompt: "chosen",
            working_dir: None,
            extra_inputs: &HashMap::new(),
            parent_invocation_env: None,
        })
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.provider_index, 9);
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "effective=chosen\n"
        );
    }
}
