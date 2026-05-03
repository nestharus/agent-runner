#![cfg(unix)]

#[path = "fixtures/b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_lib::process::{OutputSpec, StdinSpec};
use agent_runner_lib::setup::agent::SetupAgent;
use b2_process_runner::{FakeProcessRunner, ok_output};
use std::time::Duration;

/// Risk: T12 - setup-agent migration is especially prone to argument-shape
/// drift because the new runner API could tempt splitting the current combined
/// system/message prompt into multiple argv items.
/// Level: component.
/// Source: B2 contract sections 1.5 and 3 `setup::agent::send_turn`.
/// Observable: `send_turn_with_runner` preserves one final prompt arg ending
/// in `{system_prompt}\n\n---\n\n{message}` style content rather than three
/// separate args.
#[test]
fn send_turn_with_runner_preserves_single_combined_prompt_arg() {
    let runner = FakeProcessRunner::with_responses(vec![ok_output("not json", "", 0)]);
    let mut agent = SetupAgent::new();

    let _err = agent
        .send_turn_with_runner(&runner, "Install Claude", r#"{"type":"object"}"#)
        .unwrap_err();

    let call = runner.single_call();
    assert_eq!(call.program, "claude");
    assert_eq!(call.stdin, StdinSpec::Null);
    assert_eq!(call.stdout, OutputSpec::Capture);
    assert_eq!(call.stderr, OutputSpec::Capture);
    assert_eq!(call.timeout, Some(Duration::from_secs(120)));
    assert!(call.args.contains(&"-p".to_string()));
    assert!(
        call.args
            .windows(2)
            .any(|pair| pair == ["--output-format", "json"])
    );
    assert!(
        call.args
            .windows(2)
            .any(|pair| pair == ["--model", "claude-sonnet-4-6"])
    );
    assert!(
        call.args
            .windows(2)
            .any(|pair| pair == ["--allowedTools", "Read,Bash,Glob,Grep"])
    );
    assert!(call.args.contains(&"--no-session-persistence".to_string()));
    assert!(
        call.args
            .windows(2)
            .any(|pair| pair == ["--json-schema", r#"{"type":"object"}"#])
    );

    let prompt_arg = call.args.last().unwrap();
    assert!(prompt_arg.ends_with("\n\n---\n\nInstall Claude"));
    assert!(!call.args.iter().any(|arg| arg == "---"));
}

/// Risk: T12 - non-zero setup agent exits currently return a domain-specific
/// error string with stderr truncated to 500 chars; the runner must not turn
/// non-zero statuses into generic spawn errors.
/// Level: component.
/// Source: B2 contract sections 3 and 6 setup agent.
/// Observable: non-zero exit formats `Claude CLI failed (exit N): ...` and
/// truncates long stderr.
#[test]
fn send_turn_with_runner_preserves_nonzero_exit_error_and_stderr_truncation() {
    let long_stderr = format!("{}TAIL", "x".repeat(650));
    let runner = FakeProcessRunner::with_responses(vec![ok_output("", long_stderr, 17)]);
    let mut agent = SetupAgent::new();

    let err = agent
        .send_turn_with_runner(&runner, "Continue", r#"{"type":"object"}"#)
        .unwrap_err();

    assert!(err.starts_with("Claude CLI failed (exit 17): "));
    assert!(!err.contains("TAIL"));
}

/// Risk: T12 - session resume state may be lost when `send_turn` becomes a
/// wrapper around `send_turn_with_runner`.
/// Level: component.
/// Source: B2 contract section 3 setup agent.
/// Observable: session id extracted from stderr is used as `--resume` on the
/// next turn while still preserving the single combined prompt arg.
#[test]
fn send_turn_with_runner_preserves_resume_session_arg_on_next_turn() {
    let runner = FakeProcessRunner::with_responses(vec![
        ok_output(
            r#"{"status":"ok","message":"first"}"#,
            "Session ID: abc123",
            0,
        ),
        ok_output("not json", "", 0),
    ]);
    let mut agent = SetupAgent::new();

    let _ = agent.send_turn_with_runner(&runner, "First", r#"{"type":"object"}"#);
    let _ = agent.send_turn_with_runner(&runner, "Second", r#"{"type":"object"}"#);

    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[1]
            .args
            .windows(2)
            .any(|pair| pair == ["--resume", "abc123"])
    );
    assert!(calls[1].args.last().unwrap().ends_with("\n\n---\n\nSecond"));
}
