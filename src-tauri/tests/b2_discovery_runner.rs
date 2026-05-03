#![cfg(unix)]

mod fixtures;

use agent_runner_lib::discovery::discover_models_with_runner;
use agent_runner_lib::process::{OutputSpec, StdinSpec};
use fixtures::b2_process_runner::*;

/// Risk: T17 (discovery service command ordering preservation)
/// Source: proposal §8 T17; contract §3 discovery::discover_models_with_runner
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn discovery_with_runner_attempts_version_first_then_stops_after_first_parsed_models() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"claude 1.2.3\n");
    runner.push_stdout(b"claude-3-opus\nclaude-3-sonnet\n");

    let result = discover_models_with_runner("claude", &runner).unwrap();

    assert_eq!(result.cli_name, "claude");
    assert!(result.cli_version.contains("1.2.3"));
    assert!(!result.models.is_empty());
    let calls = runner.calls();
    assert_eq!(calls[0].program, "claude");
    assert_eq!(calls[0].args, vec!["--version"]);
    assert_eq!(calls[1].program, "claude");
    assert_ne!(calls[1].args, vec!["--version"]);
    assert_eq!(calls.len(), 2, "must stop after parsed models: {calls:?}");
    assert!(calls.iter().all(|call| call.stdin == StdinSpec::Null));
    assert!(calls.iter().all(|call| call.stdout == OutputSpec::Capture));
    assert!(calls.iter().all(|call| call.stderr == OutputSpec::Capture));
    assert!(calls.iter().all(|call| call.timeout.is_none()));
}

/// Risk: T17 (discovery accepts useful stderr content)
/// Source: proposal §8 T17; contract §6 stderr-only success/non-zero content
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn discovery_with_runner_uses_stderr_when_it_has_more_content() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"claude 1.2.3\n");
    runner.push_response(Ok(output(b"", b"claude-3-haiku\n", 1)));

    let result = discover_models_with_runner("claude", &runner).unwrap();

    assert_eq!(result.models.len(), 1);
    assert_eq!(result.models[0].canonical_name, "claude-3-haiku");
}

/// Risk: T17 (discovery reports failed empty command output)
/// Source: proposal §8 T17; contract §6 zero-byte stdout/non-zero discovery command
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn discovery_with_runner_errors_when_failed_strategy_has_no_output() {
    let runner = FakeProcessRunner::new();
    runner.push_stdout(b"claude 1.2.3\n");
    runner.push_response(Ok(output(b"", b"", 1)));

    let err = discover_models_with_runner("claude", &runner).unwrap_err();

    assert!(err.contains("claude") || err.contains("failed"), "{err}");
    let calls = runner.calls();
    assert_eq!(calls[0].args, vec!["--version"]);
    assert!(calls.len() >= 2);
}

/// Risk: T17 (version command spawn failure remains discovery-classified)
/// Source: proposal §8 T17; contract §6 discovery spawn failure
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn discovery_with_runner_maps_version_runner_error_to_cli_not_executable() {
    let runner = FakeProcessRunner::new();
    runner.push_error("permission denied");

    let err = discover_models_with_runner("claude", &runner).unwrap_err();

    assert!(
        err.contains("not found") || err.contains("not executable") || err.contains("permission"),
        "{err}"
    );
    let call = runner.only_call();
    assert_eq!(call.program, "claude");
    assert_eq!(call.args, vec!["--version"]);
}
