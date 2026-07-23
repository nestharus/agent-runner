//! AGE-227 executor provider-execution characterization.
//!
//! Pins E2-owned current behavior through public `executor::cli` entrypoints
//! before moving headless/provider/resume orchestration out of `cli.rs`.
//!
//! ## Declared roles
//!
//! - Test carrier: characterize AGE-227 executor provider execution behavior.
//! - Fixture helper: build temporary provider scripts and model/provider configs.
//! - Prompt helper: construct large prompt and prompt-file observations.

#![cfg(unix)]

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionCapture,
    SessionCaptureKind,
};
use oulipoly_runtime::executor::SessionCaptureMethod;
use oulipoly_runtime::executor::cli::{
    EffectiveExecuteRequest, ResumePayload, execute_effective,
    execute_effective_with_start_known_provider_session_id, execute_resume,
    execute_resume_optional_prompt,
};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const LARGE_PROMPT_BYTES: usize = 200 * 1024;
const PROMPT_INSTRUCTION_PREFIX: &str = "Follow the instructions in ";

struct FixtureScript {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn fixture_script(body: &str) -> FixtureScript {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("provider.sh");
    std::fs::write(
        &path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .expect("write provider script");
    let mut perms = std::fs::metadata(&path)
        .expect("provider script metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod provider script");
    FixtureScript { _dir: dir, path }
}

fn provider_for(script: &FixtureScript) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: "age-227-provider".to_string(),
        command: script.path.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: None,
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn model_for(provider: ProviderConfig, prompt_mode: PromptMode) -> ModelConfig {
    ModelConfig {
        name: "age-227-model".to_string(),
        prompt_mode,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    }
}

fn flag_strategy() -> ResumeStrategy {
    ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    }
}

fn big_prompt() -> String {
    "X".repeat(LARGE_PROMPT_BYTES)
}

fn prompt_file_from_instruction(instruction: &str) -> PathBuf {
    let filename = instruction
        .strip_prefix(PROMPT_INSTRUCTION_PREFIX)
        .unwrap_or_else(|| panic!("unexpected prompt instruction: {instruction}"));
    PathBuf::from(filename)
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("read lines")
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn execute_resume_optional_prompt_none_omits_prompt_argument_and_closes_stdin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let argv_dump = dir.path().join("argv.txt");
    let stdin_dump = dir.path().join("stdin.txt");
    let script = fixture_script(&format!(
        r#"printf '%s\n' "$@" > "{argv_dump}"
cat > "{stdin_dump}"
printf 'ok'"#,
        argv_dump = argv_dump.display(),
        stdin_dump = stdin_dump.display(),
    ));
    let mut provider = provider_for(&script);
    provider.args = vec!["--base".to_string()];
    let strategy = flag_strategy();
    let session_id = "11111111-1111-4111-8111-111111111111";

    let result = execute_resume_optional_prompt(
        &provider,
        3,
        PromptMode::Arg,
        None,
        None,
        None,
        ResumePayload {
            session_id,
            strategy: &strategy,
        },
    )
    .expect("resume execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.provider_index, 3);
    assert_eq!(
        read_lines(&argv_dump),
        vec!["--base", "--resume", session_id]
    );
    assert_eq!(
        std::fs::read_to_string(&stdin_dump).expect("stdin dump"),
        ""
    );
}

#[test]
fn execute_resume_large_arg_prompt_temp_file_is_removed_before_return() {
    let dir = tempfile::tempdir().expect("tempdir");
    let observed_instruction = dir.path().join("observed-instruction.txt");
    let script = fixture_script(&format!(
        r#"instruction="${{@: -1}}"
prompt_file="${{instruction#"{prefix}"}}"
test -f "$prompt_file"
printf '%s' "$instruction" > "{observed}"
printf 'resume ok'"#,
        prefix = PROMPT_INSTRUCTION_PREFIX,
        observed = observed_instruction.display(),
    ));
    let provider = provider_for(&script);
    let strategy = flag_strategy();

    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        &big_prompt(),
        Some(dir.path()),
        None,
        ResumePayload {
            session_id: "22222222-2222-4222-8222-222222222222",
            strategy: &strategy,
        },
    )
    .expect("resume execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"resume ok");
    let prompt_path = dir.path().join(prompt_file_from_instruction(
        &std::fs::read_to_string(&observed_instruction).expect("read observed instruction"),
    ));
    assert!(
        !prompt_path.exists(),
        "resume temp prompt file should be removed before execute_resume returns: {}",
        prompt_path.display()
    );
}

#[test]
fn execute_effective_large_arg_prompt_temp_file_is_removed_before_return() {
    let dir = tempfile::tempdir().expect("tempdir");
    let observed_instruction = dir.path().join("observed-instruction.txt");
    let script = fixture_script(&format!(
        r#"instruction="${{@: -1}}"
prompt_file="${{instruction#"{prefix}"}}"
test -f "$prompt_file"
printf '%s' "$instruction" > "{observed}"
printf 'effective ok'"#,
        prefix = PROMPT_INSTRUCTION_PREFIX,
        observed = observed_instruction.display(),
    ));
    let provider = provider_for(&script);
    let model = model_for(provider.clone(), PromptMode::Arg);
    let extra_inputs = HashMap::new();

    let result = execute_effective(EffectiveExecuteRequest {
        model: &model,
        provider: &provider,
        provider_index: 4,
        prompt_mode: PromptMode::Arg,
        prompt: &big_prompt(),
        working_dir: Some(dir.path()),
        models_dir: None,
        extra_inputs: &extra_inputs,
        parent_invocation_env: None,
    })
    .expect("effective execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.provider_index, 4);
    assert_eq!(result.stdout, b"effective ok");
    let prompt_path = dir.path().join(prompt_file_from_instruction(
        &std::fs::read_to_string(&observed_instruction).expect("read observed instruction"),
    ));
    assert!(
        !prompt_path.exists(),
        "effective temp prompt file should be removed before execute_effective returns: {}",
        prompt_path.display()
    );
}

#[test]
fn execute_effective_start_known_capture_preserves_supplied_session_id() {
    let script = fixture_script(
        r#"requested=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) requested="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$requested""#,
    );
    let mut provider = provider_for(&script);
    provider.args = vec!["-p".to_string()];
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        json_args: None,
        last_message_flag: None,
    });
    let model = model_for(provider.clone(), PromptMode::Arg);
    let extra_inputs = HashMap::new();
    let pinned_session = "33333333-3333-4333-8333-333333333333";

    let result = execute_effective_with_start_known_provider_session_id(
        EffectiveExecuteRequest {
            model: &model,
            provider: &provider,
            provider_index: 6,
            prompt_mode: PromptMode::Arg,
            prompt: "prompt",
            working_dir: None,
            models_dir: None,
            extra_inputs: &extra_inputs,
            parent_invocation_env: None,
        },
        Some(pinned_session),
    )
    .expect("effective execute");

    assert_eq!(result.provider_index, 6);
    assert_eq!(
        result.session_capture.session_id.as_deref(),
        Some(pinned_session)
    );
    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::ForcedFlagVerified
    ));
}
