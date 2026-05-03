#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_lib::config::{ModelConfig, PromptMode, ProviderConfig};
use agent_runner_lib::executor::cli::{self, EffectiveExecuteRequest};
use agent_runner_lib::process::{OutputSpec, StdinSpec};
use b2_process_runner::{FakeProcessRunner, ok_output};
use std::collections::HashMap;

fn model_with(provider: ProviderConfig, prompt_mode: PromptMode) -> ModelConfig {
    ModelConfig {
        name: "fixture-model".to_string(),
        prompt_mode,
        providers: vec![provider],
        inputs: Vec::new(),
    }
}

fn provider(command: &str, args: &[&str]) -> ProviderConfig {
    let mut provider = ProviderConfig::new(
        command.to_string(),
        args.iter().map(|arg| arg.to_string()).collect(),
    );
    provider.name = "fixture-provider".to_string();
    provider
}

/// Risk: T5 - executor command assembly may drift while moving direct spawn
/// into `ProcessRunner`, especially prefixed command splitting, cwd, parent
/// invocation env, output capture, and prompt-as-argv behavior.
/// Level: component.
/// Source: B2 contract sections 3 and 5.
/// Observable: `execute_with_runner` passes the exact command spec to the
/// runner and maps process output back to `ExecutionResult`.
#[test]
fn execute_with_runner_preserves_arg_prompt_command_shape() {
    let runner = FakeProcessRunner::with_responses(vec![ok_output("stdout", "stderr", 0)]);
    let provider = provider(
        "env -u CLAUDECODE fixture-cli",
        &["--base", "--model", "fixture"],
    );
    let model = model_with(provider, PromptMode::Arg);
    let cwd = tempfile::tempdir().unwrap();

    let result = cli::execute_with_runner(
        &runner,
        &model,
        0,
        "hello prompt",
        Some(cwd.path()),
        &HashMap::new(),
        Some("parent-invocation"),
    )
    .unwrap();

    assert_eq!(result.stdout, b"stdout");
    assert_eq!(result.stderr, "stderr");
    assert_eq!(result.exit_code, 0);

    let call = runner.single_call();
    assert_eq!(call.program, "env");
    assert_eq!(
        call.args,
        [
            "-u",
            "CLAUDECODE",
            "fixture-cli",
            "--base",
            "--model",
            "fixture",
            "hello prompt"
        ]
    );
    assert_eq!(call.cwd.as_deref(), Some(cwd.path()));
    assert_eq!(
        call.env
            .get("OULIPOLY_PARENT_INVOCATION")
            .map(String::as_str),
        Some("parent-invocation")
    );
    assert_eq!(call.stdin, StdinSpec::Null);
    assert_eq!(call.stdout, OutputSpec::Capture);
    assert_eq!(call.stderr, OutputSpec::Capture);
    assert_eq!(call.timeout, None);
}

/// Risk: T5 - stdin prompt mode may accidentally become another argv prompt
/// while replacing manual stdin writes with `StdinSpec`.
/// Level: component.
/// Source: B2 contract section 5 executor stdin.
/// Observable: prompt bytes are delivered through `StdinSpec::Bytes` and are
/// not appended to argv.
#[test]
fn execute_with_runner_preserves_stdin_prompt_transport() {
    let runner = FakeProcessRunner::with_responses(vec![ok_output("ok", "", 0)]);
    let model = model_with(provider("fixture-cli", &["--json"]), PromptMode::Stdin);

    cli::execute_with_runner(
        &runner,
        &model,
        0,
        "prompt over stdin",
        None,
        &HashMap::new(),
        None,
    )
    .unwrap();

    let call = runner.single_call();
    assert_eq!(call.program, "fixture-cli");
    assert_eq!(call.args, ["--json"]);
    assert_eq!(call.stdin, StdinSpec::Bytes(b"prompt over stdin".to_vec()));
}

/// Risk: T5 - large prompt handling is caller-owned; moving subprocess spawn
/// must not move temp-file lifecycle into the runner or send the whole prompt
/// as a raw argv string.
/// Level: component.
/// Source: B2 contract sections 3 and 5 executor prompt transport.
/// Observable: large arg-mode prompts are represented by the existing
/// `Follow the instructions in <file>` argv item.
#[test]
fn execute_with_runner_preserves_large_arg_prompt_file_instruction() {
    let runner = FakeProcessRunner::with_responses(vec![ok_output("ok", "", 0)]);
    let model = model_with(provider("fixture-cli", &[]), PromptMode::Arg);
    let large_prompt = "x".repeat(128 * 1024);

    cli::execute_with_runner(
        &runner,
        &model,
        0,
        &large_prompt,
        None,
        &HashMap::new(),
        None,
    )
    .unwrap();

    let call = runner.single_call();
    let prompt_arg = call.args.last().unwrap();
    assert!(prompt_arg.starts_with("Follow the instructions in "));
    assert_ne!(prompt_arg, &large_prompt);
    assert_eq!(call.stdin, StdinSpec::Null);
}

/// Risk: T5 - `execute_effective` is the runtime-provider path used by Tauri
/// and CLI callers after provider config has been merged; it can silently keep
/// a direct spawn path if only model-level execution is tested.
/// Level: component.
/// Source: B2 contract section 3 `execute_effective_with_runner`.
/// Observable: effective execution accepts the injected runner and preserves
/// prompt/cwd/env command shape.
#[test]
fn execute_effective_with_runner_uses_injected_runner() {
    let runner = FakeProcessRunner::with_responses(vec![ok_output("effective", "", 0)]);
    let provider = provider("fixture-cli", &["--effective"]);
    let cwd = tempfile::tempdir().unwrap();

    let result = cli::execute_effective_with_runner(
        &runner,
        EffectiveExecuteRequest {
            provider: &provider,
            provider_index: 2,
            prompt_mode: PromptMode::Arg,
            prompt: "effective prompt",
            working_dir: Some(cwd.path()),
            extra_inputs: &HashMap::new(),
            parent_invocation_env: Some("parent-2"),
        },
    )
    .unwrap();

    assert_eq!(result.provider_index, 2);
    let call = runner.single_call();
    assert_eq!(call.program, "fixture-cli");
    assert_eq!(call.args, ["--effective", "effective prompt"]);
    assert_eq!(call.cwd.as_deref(), Some(cwd.path()));
    assert_eq!(
        call.env
            .get("OULIPOLY_PARENT_INVOCATION")
            .map(String::as_str),
        Some("parent-2")
    );
}

/// Risk: T5/T13 - interactive execution must route through
/// `run_interactive`, not through captured `run`, and must preserve
/// interactive args, cwd, and parent invocation env.
/// Level: component.
/// Source: B2 contract section 3 `execute_interactive_with_runner`.
/// Observable: only `InteractiveCommandSpec` is recorded.
#[test]
fn execute_interactive_with_runner_uses_interactive_spec() {
    let runner = FakeProcessRunner::new();
    runner.push_interactive_response(Ok(5));
    let mut provider = provider("env -u CLAUDECODE fixture-cli", &[]);
    provider.interactive_args = Some(vec!["repl".to_string(), "--flag".to_string()]);
    let cwd = tempfile::tempdir().unwrap();

    let exit = cli::execute_interactive_with_runner(
        &runner,
        &provider,
        Some(cwd.path()),
        Some("parent-interactive"),
        None,
    )
    .unwrap();

    assert_eq!(exit, 5);
    assert!(runner.calls().is_empty());
    let call = runner.single_interactive_call();
    assert_eq!(call.program, "env");
    assert_eq!(
        call.args,
        ["-u", "CLAUDECODE", "fixture-cli", "repl", "--flag"]
    );
    assert_eq!(call.cwd.as_deref(), Some(cwd.path()));
    assert_eq!(
        call.env
            .get("OULIPOLY_PARENT_INVOCATION")
            .map(String::as_str),
        Some("parent-interactive")
    );
}
