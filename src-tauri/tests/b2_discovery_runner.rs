#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_lib::discovery;
use agent_runner_lib::process::{OutputSpec, StdinSpec};
use b2_process_runner::{FakeProcessRunner, ok_output};

/// Risk: T17 - discovery can keep direct subprocess calls if tests only assert
/// parsed output; the first command must remain the CLI version probe through
/// the runner.
/// Level: component.
/// Source: B2 contract sections 3 and 5 discovery.
/// Observable: `<cli> --version` is always the first runner call.
#[test]
fn discover_models_with_runner_attempts_cli_version_first() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("claude 1.2.3\n", "", 0),
        ok_output("", "", 0),
        ok_output("", "", 0),
        ok_output("", "", 0),
    ]);

    let _result = discovery::discover_models_with_runner("claude", &runner).unwrap();

    let calls = runner.calls();
    assert!(!calls.is_empty());
    assert_eq!(calls[0].program, "claude");
    assert_eq!(calls[0].args, ["--version"]);
    assert_eq!(calls[0].stdin, StdinSpec::Null);
    assert_eq!(calls[0].stdout, OutputSpec::Capture);
    assert_eq!(calls[0].stderr, OutputSpec::Capture);
    assert_eq!(calls[0].timeout, None);
}

/// Risk: T17 and anti-drift §1.5 - a natural refactor may "improve" discovery
/// by returning `Err` when every strategy is unsuccessful. Pre-B behavior only
/// errors when the CLI is not found; unsuccessful strategies produce `Ok(empty)`.
/// Level: component.
/// Source: B2 contract sections 1.5 and 3 discovery.
/// Observable: unsuccessful/empty strategy commands are all attempted through the
/// runner and the final result is `Ok` with no models.
#[test]
fn discover_models_with_runner_returns_ok_empty_when_all_strategies_fail() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("claude 1.2.3\n", "", 0),
        ok_output("", "", 1),
        Err("strategy two spawn refused".to_string()),
        ok_output("", "", 1),
        ok_output("", "", 0),
    ]);

    let result = discovery::discover_models_with_runner("claude", &runner).unwrap();

    assert!(result.models.is_empty());
    let calls = runner.calls();
    assert!(calls.len() > 2);
    assert!(calls[1..].iter().all(|call| call.program == "claude"));
}

/// Drift pin F-04: pre-B discovery did not filter "no models", "failed", or
/// "error" lines before extracting model-name candidates.
#[test]
fn discover_models_with_runner_keeps_no_models_and_failed_lines_as_candidates() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("claude 1.2.3\n", "", 0),
        ok_output("no models found\nclaude-haiku failed setup\n", "", 0),
    ]);

    let result = discovery::discover_models_with_runner("claude", &runner).unwrap();
    let names: Vec<_> = result
        .models
        .iter()
        .map(|model| model.canonical_name.as_str())
        .collect();

    assert_eq!(names, ["claude-haiku", "no"]);
}

/// Risk: T17 - discovery error handling must remain narrow: a missing CLI at
/// the version probe is still an error even though later strategy failures are
/// tolerated.
/// Level: component.
/// Source: B2 contract section 6 process spawn failure.
/// Observable: version-probe runner errors propagate as CLI-not-found errors.
#[test]
fn discover_models_with_runner_errors_when_cli_version_probe_fails() {
    let runner = FakeProcessRunner::with_responses(vec![Err("not executable".to_string())]);

    let err = discovery::discover_models_with_runner("missing-cli", &runner).unwrap_err();

    assert!(err.contains("missing-cli"));
    assert!(err.contains("not executable"));
    let call = runner.single_call();
    assert_eq!(call.program, "missing-cli");
    assert_eq!(call.args, ["--version"]);
}
