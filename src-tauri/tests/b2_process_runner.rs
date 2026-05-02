#![cfg(unix)]

mod fixtures;

use agent_runner_lib::process::{
    CommandSpec, InteractiveCommandSpec, OsProcessRunner, OutputSpec, ProcessRunner, StdinSpec,
};
use fixtures::b2_process_runner::*;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Risk: T5 (process-spawn seam success semantics)
/// Source: proposal §8 T5; contract §2 ProcessRunner success path
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn process_runner_run_returns_captured_stdout_stderr_and_zero_exit() {
    let runner = OsProcessRunner;
    let spec = command_spec("sh", &["-c", "printf out; printf err >&2"]);

    let output = runner.run(spec).unwrap();

    assert_eq!(output.stdout, b"out");
    assert_eq!(output.stderr, b"err");
    assert_eq!(output.exit_code, 0);
    assert!(!output.timed_out);
}

/// Risk: T5 (non-zero exits remain caller-classified)
/// Source: proposal §8 T5; contract §2 ProcessOutput non-zero behavior
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn process_runner_nonzero_exit_returns_ok_process_output() {
    let runner = OsProcessRunner;
    let spec = command_spec("sh", &["-c", "printf nope >&2; exit 7"]);

    let output = runner.run(spec).unwrap();

    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stderr, b"nope");
}

/// Risk: T5 (stdin byte transport preservation)
/// Source: proposal §8 T5; contract §2 StdinSpec::Bytes
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn process_runner_writes_exact_stdin_bytes_before_waiting() {
    let runner = OsProcessRunner;
    let mut spec = command_spec("sh", &["-c", "cat"]);
    spec.stdin = StdinSpec::Bytes(vec![0, b'a', b'\n', 0xff]);

    let output = runner.run(spec).unwrap();

    assert_eq!(output.stdout, vec![0, b'a', b'\n', 0xff]);
}

/// Risk: T5 (OutputSpec::Null discards selected streams)
/// Source: proposal §8 T5; contract §2 OutputSpec
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn process_runner_null_output_specs_return_empty_vectors() {
    let runner = OsProcessRunner;
    let mut spec = command_spec("sh", &["-c", "printf out; printf err >&2"]);
    spec.stdout = OutputSpec::Null;
    spec.stderr = OutputSpec::Null;

    let output = runner.run(spec).unwrap();

    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(output.exit_code, 0);
}

/// Risk: T5 (cwd and additive env composition)
/// Source: proposal §8 T5; contract §2 CommandSpec cwd/env
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn process_runner_applies_cwd_and_additive_env() {
    let runner = OsProcessRunner;
    let dir = tempfile::tempdir().unwrap();
    let mut spec = command_spec("sh", &["-c", "printf '%s|%s' \"$PWD\" \"$B2_SENTINEL\""]);
    spec.cwd = Some(dir.path().to_path_buf());
    spec.env = HashMap::from([("B2_SENTINEL".to_string(), "from-env".to_string())]);

    let output = runner.run(spec).unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();

    assert_eq!(stdout, format!("{}|from-env", dir.path().display()));
}

/// Risk: T5 (timeout does not leak a live child)
/// Source: proposal §8 T5; contract §6 Process timeout
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn process_runner_timeout_returns_error_instead_of_success_output() {
    let runner = OsProcessRunner;
    let mut spec = command_spec("sh", &["-c", "sleep 5"]);
    spec.timeout = Some(Duration::from_millis(100));
    spec.description = "timeout fixture".to_string();
    let started = Instant::now();

    let err = runner.run(spec).unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        err.contains("timeout") || err.contains("timed out"),
        "{err}"
    );
}

/// Risk: T5 (interactive TTY handoff preserves exit-code mapping)
/// Source: proposal §8 T5/T13; contract §2 InteractiveCommandSpec
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn process_runner_interactive_returns_child_exit_code() {
    let runner = OsProcessRunner;
    let spec = InteractiveCommandSpec {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), "exit 13".to_string()],
        cwd: None,
        env: HashMap::new(),
        description: "interactive exit fixture".to_string(),
    };

    let exit_code = runner.run_interactive(spec).unwrap();

    assert_eq!(exit_code, 13);
}

/// Risk: T5 (fake runner records owned specs before returning)
/// Source: proposal §8 T5; contract §5 fixture pattern
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn fake_process_runner_records_specs_and_respects_null_output() {
    let runner = FakeProcessRunner::new();
    runner.push_response(Ok(output(b"kept?", b"hidden?", 0)));
    let mut spec = command_spec("fixture", &["arg"]);
    spec.stdout = OutputSpec::Null;
    spec.stderr = OutputSpec::Null;

    let output = runner.run(spec).unwrap();

    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(runner.only_call().program, "fixture");
}
