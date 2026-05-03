#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_executor::{
    CommandSpec, InteractiveCommandSpec, OsProcessRunner, OutputSpec, ProcessRunner, StdinSpec,
};
use b2_process_runner::{FakeProcessRunner, ok_output};
use std::collections::HashMap;
use std::time::Duration;

/// Risk: T5/T12/T17/T18 - replacing bespoke subprocess calls with one runner
/// may accidentally turn non-zero exits into runner errors or lose command
/// request details that callers must classify themselves.
/// Level: component.
/// Source: B2 contract sections 2 and 6.
/// Observable: `ProcessRunner::run` records an owned `CommandSpec` and returns
/// non-zero exit status as `Ok(ProcessOutput)`.
#[test]
fn fake_process_runner_records_owned_command_specs_and_nonzero_output() {
    let runner = FakeProcessRunner::with_responses(vec![ok_output("out", "err", 42)]);
    let mut env = HashMap::new();
    env.insert(
        "OULIPOLY_PARENT_INVOCATION".to_string(),
        "parent-1".to_string(),
    );

    let output = runner
        .run(CommandSpec {
            program: "fixture-cli".to_string(),
            args: vec!["--flag".to_string(), "value".to_string()],
            cwd: None,
            env,
            stdin: StdinSpec::Bytes(b"stdin bytes".to_vec()),
            stdout: OutputSpec::Capture,
            stderr: OutputSpec::Capture,
            timeout: Some(Duration::from_secs(9)),
            description: "fixture command".to_string(),
        })
        .unwrap();

    assert_eq!(output.exit_code, 42);
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");

    let call = runner.single_call();
    assert_eq!(call.program, "fixture-cli");
    assert_eq!(call.args, ["--flag", "value"]);
    assert_eq!(call.stdin, StdinSpec::Bytes(b"stdin bytes".to_vec()));
    assert_eq!(call.timeout, Some(Duration::from_secs(9)));
    assert_eq!(
        call.env
            .get("OULIPOLY_PARENT_INVOCATION")
            .map(String::as_str),
        Some("parent-1")
    );
}

/// Risk: T5 - interactive execution has different stdio and signal behavior
/// from captured execution, so collapsing it into `run` would change user TTY
/// handoff semantics.
/// Level: component.
/// Source: B2 contract section 2 `InteractiveCommandSpec`.
/// Observable: interactive calls are recorded separately and return a raw exit
/// code, including `-1` for signal/unknown status.
#[test]
fn fake_process_runner_records_interactive_specs_separately() {
    let runner = FakeProcessRunner::new();
    runner.push_interactive_response(Ok(-1));

    let exit = runner
        .run_interactive(InteractiveCommandSpec {
            program: "fixture-repl".to_string(),
            args: vec!["chat".to_string()],
            cwd: None,
            env: HashMap::new(),
            description: "interactive fixture".to_string(),
        })
        .unwrap();

    assert_eq!(exit, -1);
    assert!(runner.calls().is_empty());
    assert_eq!(runner.single_interactive_call().program, "fixture-repl");
}

/// Drift pin F-05: pre-B interactive process exits matched shell convention
/// for signal deaths: 128 + signal number, not `-1`.
#[test]
fn os_process_runner_interactive_maps_signal_exit_to_128_plus_signal() {
    let exit = OsProcessRunner
        .run_interactive(InteractiveCommandSpec {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "kill -TERM $$".to_string()],
            cwd: None,
            env: HashMap::new(),
            description: "interactive signal fixture".to_string(),
        })
        .unwrap();

    assert_eq!(exit, 128 + 15);
}

/// Risk: T5/T4 - the production runner can regress pipe draining or exit-code
/// handling even when fake-runner seam tests pass.
/// Level: particular-integration.
/// Source: B2 contract section 2 `OsProcessRunner`.
/// Observable: captured stdout/stderr are returned and non-zero status remains
/// `Ok(ProcessOutput)`.
#[test]
fn os_process_runner_captures_streams_and_preserves_nonzero_exit() {
    let output = OsProcessRunner
        .run(CommandSpec {
            program: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "printf out; printf err >&2; exit 7".to_string(),
            ],
            cwd: None,
            env: HashMap::new(),
            stdin: StdinSpec::Null,
            stdout: OutputSpec::Capture,
            stderr: OutputSpec::Capture,
            timeout: Some(Duration::from_secs(5)),
            description: "os process smoke".to_string(),
        })
        .unwrap();

    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
    assert!(!output.timed_out);
}

/// Risk: T5 - stdin transport currently belongs to callers, but the runner
/// owns writing bytes once the command spec is assembled.
/// Level: particular-integration.
/// Source: B2 contract section 2 `StdinSpec`.
/// Observable: `StdinSpec::Bytes` reaches the child exactly.
#[test]
fn os_process_runner_writes_stdin_bytes_exactly() {
    let output = OsProcessRunner
        .run(CommandSpec {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "cat".to_string()],
            cwd: None,
            env: HashMap::new(),
            stdin: StdinSpec::Bytes(b"hello stdin".to_vec()),
            stdout: OutputSpec::Capture,
            stderr: OutputSpec::Capture,
            timeout: Some(Duration::from_secs(5)),
            description: "stdin smoke".to_string(),
        })
        .unwrap();

    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"hello stdin");
    assert!(output.stderr.is_empty());
}
