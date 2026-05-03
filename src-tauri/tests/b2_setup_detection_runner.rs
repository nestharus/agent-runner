#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_app::check_setup_needed_with_runner;
use agent_runner_app::setup::detection;
use agent_runner_executor::{OutputSpec, StdinSpec};
use b2_process_runner::{FakeProcessRunner, ok_output};

/// Risk: T18 - per-CLI detection can retain direct `Command::new` calls after
/// only broad `detect_all` tests pass.
/// Level: component.
/// Source: B2 contract sections 3 and 5 setup detection.
/// Observable: Claude detection probes `which`, version, and `claude auth
/// status` through the injected runner with no timeout.
#[test]
fn detect_single_claude_with_runner_uses_runner_for_version_and_auth_status() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("/usr/bin/claude\n", "", 0),
        ok_output("claude 1.2.3\n", "", 0),
        ok_output("Logged in as user@example.com\n", "", 0),
    ]);

    let info = detection::detect_single_cli_with_runner("claude", &runner);

    assert_eq!(info.name, "claude");
    let calls = runner.calls();
    assert!(calls.len() >= 3);
    assert_eq!(calls[0].program, "which");
    assert_eq!(calls[0].args, ["claude"]);
    assert_eq!(calls[0].stdin, StdinSpec::Null);
    assert_eq!(calls[0].stdout, OutputSpec::Capture);
    assert_eq!(calls[0].stderr, OutputSpec::Capture);
    assert_eq!(calls[0].timeout, None);
    assert_eq!(calls[1].program, "claude");
    assert_eq!(calls[1].args, ["--version"]);
    assert!(
        calls
            .iter()
            .any(|call| call.program == "claude" && call.args == ["auth", "status"])
    );
}

/// Risk: T18 - Codex detection has a separate profile/auth branch from Claude
/// and can be missed by a provider-agnostic detection test.
/// Level: component.
/// Source: B2 contract sections 3 and 5 setup detection.
/// Observable: Codex detection probes `codex login status` through the runner.
#[test]
fn detect_single_codex_with_runner_uses_runner_for_login_status() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("/usr/bin/codex\n", "", 0),
        ok_output("codex 0.20.0\n", "", 0),
        ok_output("Logged in\n", "", 0),
    ]);

    let info = detection::detect_single_cli_with_runner("codex", &runner);

    assert_eq!(info.name, "codex");
    let calls = runner.calls();
    assert_eq!(calls[0].program, "which");
    assert_eq!(calls[0].args, ["codex"]);
    assert!(
        calls
            .iter()
            .any(|call| call.program == "codex" && call.args == ["login", "status"])
    );
}

/// Risk: T18 - the broad `detect_all` wrapper may instantiate the OS runner
/// internally while per-CLI helpers accept the injected runner.
/// Level: component.
/// Source: B2 contract section 3 setup detection wrappers.
/// Observable: `detect_all_with_runner` fans out through the fake runner rather
/// than reading the host PATH directly.
#[test]
fn detect_all_with_runner_uses_injected_runner_for_cli_probes() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("/usr/bin/claude\n", "", 0),
        ok_output("claude 1.2.3\n", "", 0),
        ok_output("Logged in\n", "", 0),
        ok_output("", "", 1),
        ok_output("", "", 1),
        ok_output("", "", 1),
    ]);

    let _report = detection::detect_all_with_runner(&runner);

    let calls = runner.calls();
    assert!(calls.iter().any(|call| call.program == "which"));
    assert!(
        calls
            .iter()
            .any(|call| call.program == "claude" && call.args == ["--version"])
    );
}

/// Risk: T18 - tracker-aware wrappers can keep using the old no-runner helper
/// even when the simpler runner-aware functions are migrated.
/// Level: component.
/// Source: B2 contract section 3 setup detection wrappers.
/// Observable: both tracker-aware variants accept the injected runner and use
/// it for subprocess probes when the tracker is absent.
#[test]
fn tracker_aware_detection_variants_accept_runner() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output("/usr/bin/claude\n", "", 0),
        ok_output("claude 1.2.3\n", "", 0),
        ok_output("Logged in\n", "", 0),
        ok_output("/usr/bin/claude\n", "", 0),
        ok_output("claude 1.2.3\n", "", 0),
        ok_output("Logged in\n", "", 0),
        ok_output("", "", 1),
        ok_output("", "", 1),
        ok_output("", "", 1),
    ]);

    let _info = detection::detect_single_cli_with_tracker_and_runner("claude", None, &runner);
    let _report = detection::detect_all_with_tracker_and_runner(None, &runner);

    let calls = runner.calls();
    assert!(
        calls
            .iter()
            .filter(|call| call.program == "which" && call.args == ["claude"])
            .count()
            >= 2
    );
}

/// Risk: T18/Q1 - `lib.rs::check_setup_needed` was missed in the prior B
/// attempt; it must migrate from direct `which claude` to the process runner
/// without changing its setup-needed semantics.
/// Level: hookpoint.
/// Source: B2 contract sections 4 and 9 Q1.
/// Observable: the helper probes `which claude` through the injected runner
/// when models exist and reports setup not needed on success.
#[test]
fn check_setup_needed_with_runner_probes_claude_when_models_exist() {
    let runner = FakeProcessRunner::with_responses(vec![ok_output("/usr/bin/claude\n", "", 0)]);

    let needed = check_setup_needed_with_runner(&runner, false).unwrap();

    assert!(!needed);
    let call = runner.single_call();
    assert_eq!(call.program, "which");
    assert_eq!(call.args, ["claude"]);
    assert_eq!(call.stdin, StdinSpec::Null);
    assert_eq!(call.stdout, OutputSpec::Capture);
    assert_eq!(call.stderr, OutputSpec::Capture);
    assert_eq!(call.timeout, None);
}

/// Risk: T18/Q1 - the helper must keep the pre-B short-circuit for empty model
/// registries; otherwise first-run setup behavior changes even though the
/// process seam is correct.
/// Level: hookpoint.
/// Source: B2 contract sections 4 and 9 Q1.
/// Observable: empty models return setup-needed without probing `which`.
#[test]
fn check_setup_needed_with_runner_short_circuits_when_models_are_empty() {
    let runner = FakeProcessRunner::new();

    let needed = check_setup_needed_with_runner(&runner, true).unwrap();

    assert!(needed);
    assert!(runner.calls().is_empty());
}
