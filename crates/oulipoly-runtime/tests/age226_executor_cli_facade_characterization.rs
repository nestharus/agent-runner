//! AGE-226 executor CLI facade characterization.
//!
//! Pins the public behavior of E1-owned temp-file cleanup through
//! `oulipoly_runtime::executor::cli::execute`.

#![cfg(unix)]

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, SessionCapture, SessionCaptureKind,
};
use oulipoly_runtime::executor::SessionCaptureMethod;
use oulipoly_runtime::executor::cli::execute;
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

fn model_for(script: &FixtureScript, prompt_mode: PromptMode) -> ModelConfig {
    ModelConfig {
        name: "age-226-executor-cli-facade".to_string(),
        prompt_mode,
        providers: vec![ProviderConfig::new(
            script.path.to_string_lossy().into_owned(),
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: None,
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

fn read_observed_path(path: &Path) -> PathBuf {
    PathBuf::from(std::fs::read_to_string(path).expect("read observed path"))
}

/// Risk: T1 - temp cleanup behavior regresses when `cleanup_temp_files` moves.
/// Source: AGE-226 contract Test Intent Handoff row T1; proposal test-intent track.
/// Fixture: large PromptMode::Arg payload through `cli::execute`.
#[test]
fn large_arg_prompt_temp_file_is_removed_before_execute_returns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let observed_instruction = dir.path().join("observed-instruction.txt");
    let script = fixture_script(&format!(
        r#"instruction="${{1:?missing prompt instruction}}"
prompt_file="${{instruction#"{prefix}"}}"
test -f "$prompt_file"
printf '%s' "$instruction" > "{observed}"
printf 'ok'"#,
        prefix = PROMPT_INSTRUCTION_PREFIX,
        observed = observed_instruction.display(),
    ));
    let model = model_for(&script, PromptMode::Arg);

    let result = execute(
        &model,
        0,
        &big_prompt(),
        Some(dir.path()),
        &HashMap::new(),
        None,
    )
    .expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"ok");
    let prompt_path = dir.path().join(prompt_file_from_instruction(
        &std::fs::read_to_string(&observed_instruction).expect("read observed instruction"),
    ));
    assert!(
        !prompt_path.exists(),
        "large prompt temp file should be removed before execute returns: {}",
        prompt_path.display()
    );
}

/// Risk: T1 - temp cleanup behavior regresses when `cleanup_temp_files` moves.
/// Source: AGE-226 contract Test Intent Handoff row T1; proposal test-intent track.
/// Fixture: stdout-json-event session capture sidecar through `cli::execute`.
#[test]
fn stdout_json_event_last_message_sidecar_is_removed_after_stdout_mapping() {
    let dir = tempfile::tempdir().expect("tempdir");
    let observed_last_message = dir.path().join("observed-last-message.txt");
    let script = fixture_script(&format!(
        r#"
last_message_path=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --last-message) last_message_path="$2"; shift 2 ;;
    *) shift ;;
  esac
done
test -n "$last_message_path"
printf '%s' "$last_message_path" > "{observed}"
printf 'restored stdout body' > "$last_message_path"
printf '{{"type":"agent.session_started","data":{{"id":"age-226-session"}}}}\n'
"#,
        observed = observed_last_message.display(),
    ));
    let mut model = model_for(&script, PromptMode::Arg);
    model.providers[0].session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::StdoutJsonEvent,
        flag: None,
        readback_args: None,
        event_type: Some("agent.session_started".to_string()),
        event_id_path: Some("data.id".to_string()),
        json_flag: Some("--json".to_string()),
        last_message_flag: Some("--last-message".to_string()),
    });

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"restored stdout body");
    assert_eq!(
        result.session_capture.session_id.as_deref(),
        Some("age-226-session")
    );
    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::StdoutJsonEvent
    ));
    let last_message_path = read_observed_path(&observed_last_message);
    assert!(
        !last_message_path.exists(),
        "last-message sidecar should be removed after restored stdout is mapped: {}",
        last_message_path.display()
    );
}

/// Risk: T1 - temp cleanup behavior regresses when `cleanup_temp_files` moves.
/// Source: AGE-226 contract Test Intent Handoff row T1; proposal test-intent track.
/// Fixture: provider replaces the prompt temp file with a directory before cleanup.
#[test]
fn temp_file_cleanup_failure_does_not_convert_successful_execution_to_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let observed_instruction = dir.path().join("observed-instruction.txt");
    let script = fixture_script(&format!(
        r#"instruction="${{1:?missing prompt instruction}}"
prompt_file="${{instruction#"{prefix}"}}"
test -f "$prompt_file"
printf '%s' "$instruction" > "{observed}"
rm "$prompt_file"
mkdir "$prompt_file"
printf 'provider success despite cleanup failure'"#,
        prefix = PROMPT_INSTRUCTION_PREFIX,
        observed = observed_instruction.display(),
    ));
    let model = model_for(&script, PromptMode::Arg);

    let result = execute(
        &model,
        0,
        &big_prompt(),
        Some(dir.path()),
        &HashMap::new(),
        None,
    )
    .expect("cleanup failure should not turn successful provider execution into Err");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"provider success despite cleanup failure");
    let prompt_path = dir.path().join(prompt_file_from_instruction(
        &std::fs::read_to_string(&observed_instruction).expect("read observed instruction"),
    ));
    assert!(
        prompt_path.is_dir(),
        "fixture should leave a directory at the temp-file path so remove_file fails"
    );
}
