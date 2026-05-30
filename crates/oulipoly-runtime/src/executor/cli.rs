//! ## Declared roles
//!
//! Roles: orchestration, formatter, mapper, validator.
//!
//! - orchestration: top-level [`execute`], [`execute_effective`],
//!   [`execute_effective_with_start_known_provider_session_id`],
//!   [`execute_resume`], [`execute_interactive`], and
//!   [`execute_interactive_with_result`] entrypoints. This module composes
//!   the per-component submodules listed below; predicates, parsers,
//!   validators, formatters, mappers, accessors, and filters all live in
//!   sibling files under `executor/cli/`.
//! - formatter: [`interactive_args_missing_error`].
//! - mapper: [`interactive_result_from_status`].
//! - validator: [`validated_interactive_args`].
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli.rs
//!     role: adapter
//!     Translates:
//!       - executor-public-entrypoint-contract
//!       - executor-cli-component-set-contract
//!       - executor-cli-test-fixture-contract
//!       - tempfile-unix-permissions-test-contract
//! ```
//!
//! ## Component-set
//!
//! Each component is a sibling module with its own `## Declared roles`
//! header:
//!
//! - [`input_flags`] (c1) — input-schema validation and flag formatting.
//! - [`ipc`] (c6) — return-channel + child-marker IPC. Preserves env names,
//!   stderr marker prefix, and JSONL semantics bit-for-bit.
//! - [`launch`] (c3) — provider launch + command construction.
//! - [`policy`] (c4) — provider policy emission for Claude/Codex.
//! - [`provider_lookup`] — configured provider lookup and provider-index
//!   error formatting.
//! - [`provider_identity`] (c4) — `shell_split`, `provider_name`,
//!   `provider_executable_name`, and the ACR-205 intrinsic-surface
//!   declaration for the bounded `claude | codex | openai_compat`
//!   provider set.
//! - [`request`] — public effective execution request carrier.
//! - [`result`] — maps supervised output and return-channel artifacts onto
//!   executor result DTOs, plus prompt/session temp-file cleanup.
//! - [`resume`] (c5) — resume args composition + acceptance classification.
//!   Carries the ACR-251 canonical-doc-as-schema declaration for PP-009.
//! - [`capture_result`] (c5) — maps capture plans and parsed provider output
//!   onto executor session-capture result DTOs.
//! - [`session_capture`] (c5) — `start_known_provider_session_id`, capture
//!   plans, and stdout JSONL parsers. Carries the ACR-251 canonical-doc-
//!   as-schema declarations for PP-007 + PP-008.
//! - [`supervision`] (c3) — provider supervisor + child IO.
//! - [`terminal_signal`] (c3) — terminal reason classifier and the
//!   interactive signal-forwarding guard.

mod capture_result;
mod input_flags;
mod ipc;
mod launch;
mod policy;
mod provider_identity;
mod provider_lookup;
mod request;
mod result;
mod resume;
mod session_capture;
mod supervision;
mod terminal_signal;

pub use provider_identity::{provider_name, shell_split};
pub use request::EffectiveExecuteRequest;
pub use resume::{ResumePayload, compose_resume_args};
pub use session_capture::start_known_provider_session_id;
pub use terminal_signal::classify_terminal_reason;

use input_flags::resolve_input_flags;
use launch::{ProviderLaunch, ProviderLaunchRequest, assemble_provider_launch, build_command};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use policy::apply_provider_policy;
use provider_lookup::provider_for_index;
use result::{
    RawResult, cleanup_temp_files, execution_result_from_raw, raw_result_from_supervised_output,
};
use resume::{classify_resume_acceptance, compose_resume_provider_args};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use supervision::{SupervisorConfig, run_provider_supervisor};

use super::{ExecutionResult, SessionCaptureMethod, SessionCaptureResult, TerminalSignal};

pub fn execute(
    model: &ModelConfig,
    provider_index: usize,
    prompt: &str,
    working_dir: Option<&Path>,
    extra_inputs: &HashMap<String, Vec<String>>,
    parent_invocation_env: Option<&str>,
) -> Result<ExecutionResult, String> {
    let provider = provider_for_index(model, provider_index)?;
    let input_args = resolve_input_flags(model, extra_inputs)?;
    let (result, temp_files) = execute_provider(
        provider,
        model.prompt_mode,
        prompt,
        working_dir,
        &input_args,
        parent_invocation_env,
        None,
    )?;
    cleanup_temp_files(temp_files);

    Ok(execution_result_from_raw(
        result,
        provider_index,
        None,
        None,
    ))
}

pub fn execute_effective(request: EffectiveExecuteRequest<'_>) -> Result<ExecutionResult, String> {
    execute_effective_with_start_known_provider_session_id(request, None)
}

pub fn execute_effective_with_start_known_provider_session_id(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
) -> Result<ExecutionResult, String> {
    execute_effective_with_optional_supervisor_config(
        request,
        start_known_provider_session_id,
        None,
    )
}

#[cfg(test)]
fn execute_effective_with_supervisor_config(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
    supervisor_config: SupervisorConfig,
) -> Result<ExecutionResult, String> {
    execute_effective_with_optional_supervisor_config(
        request,
        start_known_provider_session_id,
        Some(supervisor_config),
    )
}

fn execute_effective_with_optional_supervisor_config(
    request: EffectiveExecuteRequest<'_>,
    start_known_provider_session_id: Option<&str>,
    supervisor_config: Option<SupervisorConfig>,
) -> Result<ExecutionResult, String> {
    let input_args = resolve_input_flags(request.model, request.extra_inputs)?;
    let (result, temp_files) = execute_provider_with_arg_parts_and_supervisor_config(
        request.provider,
        &request.provider.args,
        &[],
        request.prompt_mode,
        Some(request.prompt),
        request.working_dir,
        &input_args,
        request.parent_invocation_env,
        start_known_provider_session_id,
        supervisor_config,
    )?;
    cleanup_temp_files(temp_files);

    Ok(execution_result_from_raw(
        result,
        request.provider_index,
        None,
        None,
    ))
}

fn execute_provider(
    provider: &ProviderConfig,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    execute_provider_with_arg_parts_and_supervisor_config(
        provider,
        &provider.args,
        &[],
        prompt_mode,
        Some(prompt),
        working_dir,
        input_args,
        parent_invocation_env,
        start_known_provider_session_id,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "executor call shape keeps base args, lifecycle tail args, prompt mode, cwd, input flags, parent marker, optional start-bound session, and optional supervisor test injection explicit"
)]
fn execute_provider_with_arg_parts_and_supervisor_config(
    provider: &ProviderConfig,
    provider_args: &[String],
    tail_args: &[String],
    prompt_mode: PromptMode,
    prompt: Option<&str>,
    working_dir: Option<&Path>,
    input_args: &[String],
    parent_invocation_env: Option<&str>,
    start_known_provider_session_id: Option<&str>,
    supervisor_config: Option<SupervisorConfig>,
) -> Result<(RawResult, Vec<PathBuf>), String> {
    let launch = assemble_provider_launch(
        ProviderLaunchRequest {
            provider,
            provider_args,
            tail_args,
            prompt_mode,
            prompt,
            working_dir,
            input_args,
            parent_invocation_env,
            start_known_provider_session_id,
        },
        supervisor_config,
    )?;
    let ProviderLaunch {
        cmd,
        supervisor_config,
        capture_plan,
        return_channel,
        temp_files,
    } = launch;
    let output = run_provider_supervisor(cmd, provider, supervisor_config)?;
    let returned_artifacts = ipc::read_and_cleanup_return_channel(&return_channel);
    let result = raw_result_from_supervised_output(&capture_plan, output, returned_artifacts);

    Ok((result, temp_files))
}

pub fn execute_resume(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: &str,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
) -> Result<ExecutionResult, String> {
    execute_resume_optional_prompt(
        provider,
        provider_index,
        prompt_mode,
        Some(prompt),
        working_dir,
        parent_invocation_env,
        resume,
    )
}

pub fn execute_resume_optional_prompt(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: Option<&str>,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
) -> Result<ExecutionResult, String> {
    execute_resume_with_optional_supervisor_config(
        provider,
        provider_index,
        prompt_mode,
        prompt,
        working_dir,
        parent_invocation_env,
        resume,
        None,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "resume execution keeps provider, prompt, cwd, parent marker, resume payload, and optional supervisor test injection explicit"
)]
fn execute_resume_with_optional_supervisor_config(
    provider: &ProviderConfig,
    provider_index: usize,
    prompt_mode: PromptMode,
    prompt: Option<&str>,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: ResumePayload<'_>,
    supervisor_config: Option<SupervisorConfig>,
) -> Result<ExecutionResult, String> {
    let session_id = resume.session_id.to_string();
    let resume_args = compose_resume_args(resume.strategy, resume.session_id)?;
    let mut provider_without_capture = provider.clone();
    provider_without_capture.session_capture = None;
    let (result, temp_files) = execute_provider_with_arg_parts_and_supervisor_config(
        &provider_without_capture,
        &provider.args,
        &resume_args,
        prompt_mode,
        prompt,
        working_dir,
        &[],
        parent_invocation_env,
        None,
        supervisor_config,
    )?;
    cleanup_temp_files(temp_files);
    let resume_acceptance = classify_resume_acceptance(
        provider.resume_acceptance.as_ref(),
        result.exit_code,
        &result.telemetry_stdout,
        result.stderr.as_bytes(),
        &session_id,
    );
    Ok(execution_result_from_raw(
        result,
        provider_index,
        Some(resume_acceptance),
        Some(SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        }),
    ))
}

pub fn execute_interactive(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
) -> Result<i32, String> {
    execute_interactive_with_result(provider, working_dir, parent_invocation_env, resume)
        .map(|result| result.exit_code)
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractiveExecutionResult {
    pub exit_code: i32,
    pub terminal_reason: Option<String>,
    pub terminal_signal: Option<TerminalSignal>,
}

pub fn execute_interactive_with_result(
    provider: &ProviderConfig,
    working_dir: Option<&Path>,
    parent_invocation_env: Option<&str>,
    resume: Option<ResumePayload<'_>>,
) -> Result<InteractiveExecutionResult, String> {
    let mut provider_args = validated_interactive_args(provider)?;
    let mut no_prompt = None;
    apply_provider_policy(provider, &mut provider_args, &mut no_prompt)?;
    if let Some(resume) = resume {
        provider_args = compose_resume_provider_args(provider_args, resume)?;
    }

    let mut cmd = build_command(
        provider,
        &provider_args,
        working_dir,
        parent_invocation_env,
        None,
    )?;
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn '{}': {e}", provider.command))?;

    #[cfg(unix)]
    let signal_guard = terminal_signal::InteractiveSignalGuard::install(&mut child)?;

    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for process: {e}"))?;

    #[cfg(unix)]
    drop(signal_guard);

    Ok(interactive_result_from_status(provider, &status))
}

fn validated_interactive_args(provider: &ProviderConfig) -> Result<Vec<String>, String> {
    provider
        .interactive_args
        .clone()
        .ok_or_else(|| interactive_args_missing_error(provider))
}

fn interactive_args_missing_error(provider: &ProviderConfig) -> String {
    format!(
        "provider {} has no interactive_args; cannot launch interactively",
        provider.name
    )
}

fn interactive_result_from_status(
    provider: &ProviderConfig,
    status: &ExitStatus,
) -> InteractiveExecutionResult {
    let terminal_reason = classify_terminal_reason(status);
    let terminal_signal = terminal_signal::recognize_terminal_signal(
        &provider.name,
        provider_identity::ProviderRecognizer::for_provider(provider),
        &[],
        &[],
        terminal_signal::terminal_status_from_exit_status(status),
    );
    InteractiveExecutionResult {
        exit_code: terminal_signal::exit_code_from_status(status),
        terminal_reason,
        terminal_signal: Some(terminal_signal),
    }
}

#[cfg(test)]
mod tests {
    //! Supervisor regression suite (AGE-141 retained coverage).
    //!
    //! These tests need access to private orchestration helpers
    //! (`execute_effective_with_supervisor_config`,
    //! `SupervisorConfig`), so they live beside the orchestrator.
    //! Public-API tests live in `crates/oulipoly-runtime/tests/`.

    use super::*;
    use crate::executor::terminal_signal::TerminalSignalKind;
    use oulipoly_config::{ProviderConfig, SessionCapture, SessionCaptureKind};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    struct FixtureScript {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    fn fixture_script(body: &str) -> FixtureScript {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture-provider.sh");
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

    fn age141_supervisor_config() -> SupervisorConfig {
        SupervisorConfig {
            prompt_mode: PromptMode::Arg,
            prompt_payload: None,
            recognizer: provider_identity::ProviderRecognizer::Claude,
        }
    }

    fn age141_model_for_provider(provider: ProviderConfig, prompt_mode: PromptMode) -> ModelConfig {
        ModelConfig {
            name: "age-141-fixture-model".to_string(),
            prompt_mode,
            providers: vec![provider],
            inputs: Vec::new(),
            provider: None,
        }
    }

    fn age141_provider(script: &FixtureScript) -> ProviderConfig {
        ProviderConfig {
            name: "claude".to_string(),
            command: script.path.to_string_lossy().into_owned(),
            args: Vec::new(),
            interactive_args: Some(Vec::new()),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }
    }

    #[cfg(unix)]
    fn age141_execute_script_with_config(
        script: &FixtureScript,
        config: SupervisorConfig,
    ) -> ExecutionResult {
        let provider = age141_provider(script);
        let model = age141_model_for_provider(provider.clone(), PromptMode::Arg);
        let extra_inputs = HashMap::new();

        execute_effective_with_supervisor_config(
            EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "age-141 prompt",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: None,
            },
            None,
            config,
        )
        .unwrap()
    }

    fn age141_signal(
        signal: &Option<TerminalSignal>,
        expected_kind: TerminalSignalKind,
    ) -> &TerminalSignal {
        let signal = signal
            .as_ref()
            .expect("AGE-141 paths must carry TerminalSignal evidence");
        assert_eq!(signal.kind, expected_kind);
        signal
    }

    #[cfg(unix)]
    #[test]
    fn t05_interactive_silent_child_does_not_use_headless_helper() {
        let script = fixture_script("exit 0");
        let provider = age141_provider(&script);
        let result = execute_interactive_with_result(&provider, None, None, None).unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.terminal_reason, None);
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }

    #[cfg(unix)]
    #[test]
    fn t06_repl_interactive_posture_has_no_idle_timeout() {
        let script = fixture_script("sleep 0.2\nexit 0");
        let provider = age141_provider(&script);
        let started = Instant::now();

        let exit_code = execute_interactive(&provider, None, None, None).unwrap();

        assert_eq!(exit_code, 0);
        assert!(
            started.elapsed() >= Duration::from_millis(180),
            "interactive path must preserve child wait posture rather than applying headless idle timeout"
        );
    }

    #[cfg(unix)]
    #[test]
    fn t07_terminal_signal_clean_exit() {
        let script = fixture_script("exit 0");

        let result = age141_execute_script_with_config(&script, age141_supervisor_config());

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.terminal_reason, None);
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }

    #[cfg(unix)]
    #[test]
    fn t08_terminal_signal_nonzero_exit() {
        let script = fixture_script("exit 42");

        let result = age141_execute_script_with_config(&script, age141_supervisor_config());

        assert_eq!(result.exit_code, 42);
        assert_eq!(result.terminal_reason.as_deref(), Some("exit_nonzero"));
        age141_signal(&result.terminal_signal, TerminalSignalKind::NonzeroExit);
    }

    #[cfg(unix)]
    #[test]
    fn t09_terminal_signal_unix_signal_exit() {
        let script = fixture_script("kill -TERM $$");

        let result = age141_execute_script_with_config(&script, age141_supervisor_config());

        assert_eq!(result.exit_code, 143);
        assert_eq!(result.terminal_reason.as_deref(), Some("signal:SIGTERM"));
        age141_signal(&result.terminal_signal, TerminalSignalKind::SignalExit);
    }

    #[cfg(unix)]
    #[test]
    fn t10_terminal_signal_spawn_error_preserves_public_error() {
        let provider = ProviderConfig {
            name: "claude".to_string(),
            command: "/definitely/not/a/real/age-141-command".to_string(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        let model = age141_model_for_provider(provider.clone(), PromptMode::Arg);
        let extra_inputs = HashMap::new();

        let err = execute_effective_with_supervisor_config(
            EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "prompt",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: None,
            },
            None,
            age141_supervisor_config(),
        )
        .unwrap_err();

        assert!(err.contains("Failed to spawn"), "{err}");
        let signal = terminal_signal::terminal_signal_for_spawn_error("claude", &err);
        assert_eq!(signal.kind, TerminalSignalKind::SpawnError);
        assert_eq!(signal.provider_name, "claude");
    }

    #[cfg(unix)]
    #[test]
    fn t12_legacy_quota_text_preserves_clean_and_nonzero_exit() {
        for (exit_code, body, expected_kind, expected_reason) in [
            (
                0,
                "printf 'Claude usage limit reached; resets at 2026-05-18T10:00:00Z\\n'\nexit 0",
                TerminalSignalKind::CleanExit,
                None,
            ),
            (
                1,
                "printf 'Claude usage limit reached; resets at 2026-05-18T10:00:00Z\\n' >&2\nexit 1",
                TerminalSignalKind::NonzeroExit,
                Some("exit_nonzero"),
            ),
        ] {
            let script = fixture_script(body);

            let result = age141_execute_script_with_config(&script, age141_supervisor_config());

            assert_eq!(
                result.exit_code, exit_code,
                "fixture should still expose the natural exit code"
            );
            assert_eq!(
                result.terminal_reason.as_deref(),
                expected_reason,
                "legacy quota substrings are not authoritative terminal signals"
            );
            age141_signal(&result.terminal_signal, expected_kind);
        }
    }

    #[cfg(unix)]
    #[test]
    fn t14_binary_stdout_preserved_under_supervisor() {
        let script = fixture_script("printf 'raw\\000\\377Z'");

        let result = age141_execute_script_with_config(&script, age141_supervisor_config());

        assert_eq!(result.stdout, b"raw\0\xffZ".to_vec());
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }

    #[cfg(unix)]
    #[test]
    fn t17_session_capture_normal_drain_carries_clean_signal() {
        let script = fixture_script(
            r#"requested=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id)
      requested="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$requested""#,
        );
        let mut provider = age141_provider(&script);
        provider.args = vec!["-p".to_string()];
        provider.session_capture = Some(SessionCapture {
            kind: SessionCaptureKind::ForcedFlagVerified,
            flag: Some("--session-id".to_string()),
            readback_args: Some(vec!["--verbose".to_string()]),
            event_type: None,
            event_id_path: None,
            json_flag: None,
            last_message_flag: None,
        });
        let model = age141_model_for_provider(provider.clone(), PromptMode::Arg);
        let extra_inputs = HashMap::new();

        let result = execute_effective_with_supervisor_config(
            EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Arg,
                prompt: "prompt",
                working_dir: None,
                extra_inputs: &extra_inputs,
                parent_invocation_env: None,
            },
            None,
            age141_supervisor_config(),
        )
        .unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(matches!(
            result.session_capture.method,
            SessionCaptureMethod::ForcedFlagVerified
        ));
        assert!(result.session_capture.session_id.is_some());
        age141_signal(&result.terminal_signal, TerminalSignalKind::CleanExit);
    }
}
