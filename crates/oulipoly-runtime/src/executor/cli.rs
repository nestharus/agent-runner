//! ## Declared roles
//!
//! Roles: orchestration, mapper, formatter, validator.
//!
//! - orchestration: top-level [`execute`], [`execute_effective`],
//!   [`execute_effective_with_start_known_provider_session_id`],
//!   [`execute_resume`], [`execute_interactive`], and
//!   [`execute_interactive_with_result`] public entrypoints. This module owns
//!   the facade re-exports and composes the per-component submodules listed
//!   below; predicates, parsers, validators, formatters, mappers, accessors,
//!   and filters all live in sibling files under `executor/cli/`.
//! - mapper: embedded test helpers map fixture models/providers.
//! - formatter: embedded test helpers format fixture scripts.
//! - validator: embedded tests assert terminal-signal evidence.
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
//! - [`policy`] (c4) — provider policy emission for configured tool families.
//! - [`provider_lookup`] — configured provider lookup and provider-index
//!   error formatting.
//! - [`provider_identity`] (c4) — `shell_split`, `provider_name`,
//!   `provider_executable_name`, and the ACR-205 intrinsic-surface
//!   declaration for the bounded recognizer set and policy-kind predicate.
//! - [`request`] — public effective execution request carrier.
//! - [`result`] — maps supervised output and return-channel artifacts onto
//!   executor result DTOs, plus prompt/session temp-file cleanup.
//! - [`resume`] (c5) — resume args composition + acceptance classification.
//!   Carries the ACR-251 canonical-doc-as-schema declaration for PP-009.
//! - [`capture_result`] (c5) — maps capture plans and parsed provider output
//!   onto executor session-capture result DTOs.
//! - [`headless`] — headless public execution entrypoints and effective
//!   execution orchestration.
//! - [`interactive`] — interactive public execution entrypoints, validation,
//!   direct spawn/wait posture, Unix signal-guard callsite, and result mapping.
//! - [`session_capture`] (c5) — `start_known_provider_session_id`, capture
//!   plans, and stdout JSONL parsers. Carries the ACR-251 canonical-doc-
//!   as-schema declarations for PP-007 + PP-008.
//! - [`provider_execution`] — provider launch/supervisor/return-channel
//!   orchestration for headless and resume execution.
//! - [`resume_execution`] — resume public entrypoints and resume execution
//!   orchestration.
//! - [`supervision`] (c3) — provider supervisor + child IO.
//! - [`terminal_signal`] (c3) — terminal reason classifier and the
//!   interactive signal-forwarding guard.

mod capture_result;
mod headless;
mod input_flags;
mod interactive;
mod ipc;
mod launch;
mod policy;
mod provider_execution;
mod provider_identity;
mod provider_lookup;
#[cfg(unix)]
pub mod pty_broker;
mod request;
mod result;
mod resume;
mod resume_execution;
mod session_capture;
pub(crate) mod spawn_identity;
mod supervision;
mod terminal_signal;

pub use headless::{
    execute, execute_effective, execute_effective_with_start_known_provider_session_id,
};
pub(crate) use input_flags::resolve_input_flags;
pub use interactive::{
    InteractiveExecutionResult, execute_interactive, execute_interactive_with_result,
    execute_interactive_with_result_and_model_config,
    execute_interactive_with_result_and_model_identity,
};
pub use provider_identity::{provider_name, shell_split};
pub use request::EffectiveExecuteRequest;
pub use resume::{ResumePayload, compose_resume_args};
pub use resume_execution::{
    execute_resume, execute_resume_optional_prompt,
    execute_resume_optional_prompt_with_model_identity,
};
pub use session_capture::start_known_provider_session_id;
pub use terminal_signal::classify_terminal_reason;
pub(crate) use terminal_signal::{
    terminal_exit_code_from_signal, terminal_reason_from_signal_status,
};

#[cfg(test)]
use super::TerminalSignal;
#[cfg(test)]
use headless::execute_effective_with_supervisor_config;

#[cfg(test)]
mod tests {
    //! Supervisor regression suite (AGE-141 retained coverage).
    //!
    //! These tests need access to private orchestration helpers
    //! (`execute_effective_with_supervisor_config`,
    //! `SupervisorConfig`), so they live beside the orchestrator.
    //! Public-API tests live in `crates/oulipoly-runtime/tests/`.

    use super::*;
    use crate::executor::cli::supervision::SupervisorConfig;
    use crate::executor::terminal_signal::TerminalSignalKind;
    use crate::executor::{ExecutionResult, SessionCaptureMethod};
    use oulipoly_config::{
        ModelConfig, PromptMode, ProviderConfig, SessionCapture, SessionCaptureKind,
    };
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
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
            recognizer: provider_identity::ProviderRecognizer::OpenAiCompat,
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
            environment: Default::default(),
            unset_environment: Default::default(),
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
                models_dir: None,
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
            environment: Default::default(),
            unset_environment: Default::default(),
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
                models_dir: None,
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
            json_args: None,
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
                models_dir: None,
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
