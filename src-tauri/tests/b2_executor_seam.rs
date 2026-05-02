#![cfg(unix)]

mod fixtures;

use agent_runner_lib::config::{PromptMode, ResumeKind, ResumeStrategy};
use agent_runner_lib::executor::cli::{
    ResumePayload, execute_interactive_with_runner, execute_resume_with_runner, execute_with_runner,
};
use agent_runner_lib::process::{OutputSpec, StdinSpec};
use fixtures::b2_process_runner::*;
use std::collections::HashMap;

/// Risk: T5 (executor non-interactive command spec preservation)
/// Source: proposal §8 T5; contract §3 executor::cli execute_with_runner
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn executor_run_with_runner_preserves_prefixed_command_args_cwd_env_and_capture() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"ok");
    let tempdir = tempfile::tempdir().unwrap();
    let model = model_with_provider(
        "fixture-model",
        PromptMode::Arg,
        provider("env -u CLAUDECODE fixture-cli", &["--fixed"]),
    );

    let result = execute_with_runner(
        &runner,
        &model,
        0,
        "hello prompt",
        Some(tempdir.path()),
        &HashMap::new(),
        Some("parent-uuid"),
    )
    .unwrap();

    assert_eq!(result.stdout, b"ok");
    let call = runner.only_call();
    assert_eq!(call.program, "env");
    assert_eq!(call.cwd.as_deref(), Some(tempdir.path()));
    assert_eq!(
        call.env
            .get("OULIPOLY_PARENT_INVOCATION")
            .map(String::as_str),
        Some("parent-uuid")
    );
    assert_eq!(call.stdout, OutputSpec::Capture);
    assert_eq!(call.stderr, OutputSpec::Capture);
    assert_eq!(call.timeout, None);
    assert!(
        call.args
            .starts_with(&["-u".into(), "CLAUDECODE".into(), "fixture-cli".into()])
    );
    assert!(call.args.iter().any(|arg| arg == "--fixed"));
    assert_eq!(call.args.last().map(String::as_str), Some("hello prompt"));
}

/// Risk: T5 (executor stdin prompt transport remains above runner)
/// Source: proposal §8 T5; contract §5 executor stdin handoff
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn executor_run_with_runner_sends_stdin_prompt_as_stdin_spec_bytes() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"stdin-ok");
    let model = model_with_provider(
        "fixture-model",
        PromptMode::Stdin,
        provider("fixture-cli", &["--from-model"]),
    );

    let result = execute_with_runner(
        &runner,
        &model,
        0,
        "piped input",
        None,
        &HashMap::new(),
        None,
    )
    .unwrap();

    assert_eq!(result.stdout, b"stdin-ok");
    let call = runner.only_call();
    assert_eq!(call.stdin, StdinSpec::Bytes(b"piped input".to_vec()));
    assert!(!call.args.iter().any(|arg| arg == "piped input"));
}

/// Risk: T5 (executor large prompt temp-file lifecycle remains executor-owned)
/// Source: proposal §8 T5; contract §5 executor prompt transport
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn executor_run_with_runner_large_arg_prompt_uses_temp_file_instruction_arg() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"large-ok");
    let prompt = "x".repeat(70_000);
    let model = model_with_provider(
        "fixture-model",
        PromptMode::Arg,
        provider("fixture-cli", &[]),
    );

    execute_with_runner(&runner, &model, 0, &prompt, None, &HashMap::new(), None).unwrap();

    let call = runner.only_call();
    let prompt_arg = call.args.last().unwrap();
    assert!(
        prompt_arg.starts_with("Follow the instructions in "),
        "{prompt_arg}"
    );
    assert!(!prompt_arg.contains(&prompt[..128]));
}

/// Risk: T5 (executor preserves runner spawn-error category)
/// Source: proposal §8 T5; contract §6 process spawn failure
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn executor_run_with_runner_maps_runner_error_to_provider_spawn_failure() {
    let runner = FakeProcessRunner::new();
    runner.push_error("spawn denied");
    let model = model_with_provider(
        "fixture-model",
        PromptMode::Arg,
        provider("fixture-cli", &[]),
    );

    let err =
        execute_with_runner(&runner, &model, 0, "hello", None, &HashMap::new(), None).unwrap_err();

    assert!(
        err.contains("Failed to spawn") || err.contains("spawn denied"),
        "{err}"
    );
    assert!(
        err.contains("fixture-cli") || err.contains("spawn denied"),
        "{err}"
    );
}

/// Risk: T5/T13 (resume execution command-shape preservation)
/// Source: proposal §8 T5/T13; contract §3 execute_resume_with_runner
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn executor_resume_with_runner_appends_resume_args_before_prompt() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"resume-ok");
    let provider = resumable_provider("fixture-cli");
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };

    let result = execute_resume_with_runner(
        &runner,
        &provider,
        0,
        PromptMode::Arg,
        "resume prompt",
        None,
        Some("parent-uuid"),
        ResumePayload {
            session_id: "session-123",
            strategy: &strategy,
            target_jsonl_path: None,
        },
    )
    .unwrap();

    assert_eq!(result.stdout, b"resume-ok");
    let call = runner.only_call();
    let resume_pos = call.args.iter().position(|arg| arg == "--resume").unwrap();
    let prompt_pos = call
        .args
        .iter()
        .position(|arg| arg == "resume prompt")
        .unwrap();
    assert!(resume_pos < prompt_pos, "{:?}", call.args);
    assert_eq!(call.args[resume_pos + 1], "session-123");
    assert_eq!(
        call.env
            .get("OULIPOLY_PARENT_INVOCATION")
            .map(String::as_str),
        Some("parent-uuid")
    );
}

/// Risk: T5/T13 (interactive execution uses TTY runner mode)
/// Source: proposal §8 T5/T13; contract §3 execute_interactive_with_runner
/// Level: unit
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn executor_interactive_with_runner_records_interactive_spec_not_capture_spec() {
    let runner = FakeProcessRunner::new();
    runner.push_interactive_response(Ok(17));
    let provider = interactive_provider("fixture-cli", &["repl", "--fast"]);
    let tempdir = tempfile::tempdir().unwrap();

    let exit_code = execute_interactive_with_runner(
        &runner,
        &provider,
        Some(tempdir.path()),
        Some("parent-uuid"),
        None,
    )
    .unwrap();

    assert_eq!(exit_code, 17);
    assert!(runner.calls().is_empty());
    let call = runner.only_interactive_call();
    assert_eq!(call.program, "fixture-cli");
    assert_eq!(call.args, vec!["repl", "--fast"]);
    assert_eq!(call.cwd.as_deref(), Some(tempdir.path()));
    assert_eq!(
        call.env
            .get("OULIPOLY_PARENT_INVOCATION")
            .map(String::as_str),
        Some("parent-uuid")
    );
}
