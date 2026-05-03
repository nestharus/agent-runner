#![cfg(unix)]

mod fixtures;

use agent_runner_lib::process::{OutputSpec, StdinSpec};
use agent_runner_lib::setup::actions::AgentAction;
use agent_runner_lib::setup::agent::SetupAgent;
use fixtures::b2_process_runner::*;
use std::time::Duration;

/// Risk: T12 (setup agent command-shape preservation)
/// Source: proposal §8 T12; contract §3 setup::agent::send_turn_with_runner
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn setup_agent_send_turn_with_runner_uses_schema_json_mode_and_parses_actions() {
    let runner = FakeProcessRunner::new();
    runner.push_response(Ok(output(
        br#"{"actions":[{"type":"status","message":"Ready"}],"done":true}"#,
        b"Session: setup-session-123\n",
        0,
    )));
    let mut agent = SetupAgent::new("system prompt".to_string());

    let result = agent
        .send_turn_with_runner(&runner, "begin setup", r#"{"type":"object"}"#)
        .unwrap();

    assert!(result.done);
    assert_eq!(result.actions.len(), 1);
    match &result.actions[0] {
        AgentAction::Status { message } => assert_eq!(message, "Ready"),
        other => panic!("expected status action, got {other:?}"),
    }
    assert_eq!(agent.session_id(), Some("setup-session-123"));
    let call = runner.only_call();
    assert_eq!(call.program, "claude");
    assert_eq!(call.stdin, StdinSpec::Null);
    assert_eq!(call.stdout, OutputSpec::Capture);
    assert_eq!(call.stderr, OutputSpec::Capture);
    assert_eq!(call.timeout, Some(Duration::from_secs(120)));
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
    assert!(
        call.args
            .iter()
            .any(|arg| arg == "--no-session-persistence")
    );
    assert!(
        call.args
            .windows(2)
            .any(|pair| pair == ["--json-schema", r#"{"type":"object"}"#])
    );
    assert!(!call.args.iter().any(|arg| arg == "--resume"));
    assert_eq!(
        call.args.last().map(String::as_str),
        Some("system prompt\n\n---\n\nbegin setup")
    );
}

/// Risk: T12 (setup agent session-id extraction stays outside runner)
/// Source: proposal §8 T12; contract §3 setup::agent session resume
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn setup_agent_send_turn_with_runner_resumes_with_learned_session_id() {
    let runner = FakeProcessRunner::new();
    runner.push_response(Ok(output(
        br#"{"actions":[],"done":false}"#,
        b"Session: setup-session-123\n",
        0,
    )));
    runner.push_response(Ok(output(br#"{"actions":[],"done":true}"#, b"", 0)));
    let mut agent = SetupAgent::new("system prompt".to_string());

    agent
        .send_turn_with_runner(&runner, "first turn", "{}")
        .unwrap();
    agent
        .send_turn_with_runner(&runner, "second turn", "{}")
        .unwrap();

    let calls = runner.calls();
    assert_eq!(calls.len(), 2);
    let second_args = &calls[1].args;
    assert!(
        second_args
            .windows(2)
            .any(|pair| pair == ["--resume", "setup-session-123"]),
        "{second_args:?}"
    );
    assert_eq!(second_args.last().map(String::as_str), Some("second turn"));
    assert!(!second_args.last().unwrap().contains("system prompt"));
}

/// Risk: T12 (setup agent non-zero exit error shape)
/// Source: proposal §8 T12; contract §3 setup::agent non-zero exit
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn setup_agent_send_turn_with_runner_nonzero_exit_truncates_stderr() {
    let runner = FakeProcessRunner::new();
    let stderr = format!("{}tail", "x".repeat(600));
    runner.push_response(Ok(output(b"", stderr.as_bytes(), 7)));
    let mut agent = SetupAgent::new("system prompt".to_string());

    let err = agent
        .send_turn_with_runner(&runner, "begin setup", "{}")
        .unwrap_err();

    assert!(err.contains("Claude CLI failed (exit 7)"), "{err}");
    assert!(err.len() < 620, "{err}");
    assert!(!err.contains("tail"), "{err}");
}

/// Risk: T12 (setup agent timeout error category)
/// Source: proposal §8 T12; contract §6 setup agent timeout
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn setup_agent_send_turn_with_runner_timeout_returns_existing_timeout_message() {
    let runner = FakeProcessRunner::new();
    runner.push_error("Claude CLI timed out after 120 seconds");
    let mut agent = SetupAgent::new("system prompt".to_string());

    let err = agent
        .send_turn_with_runner(&runner, "begin setup", "{}")
        .unwrap_err();

    assert!(
        err.contains("Claude CLI timed out after 120 seconds"),
        "{err}"
    );
    let call = runner.only_call();
    assert_eq!(call.timeout, Some(Duration::from_secs(120)));
}
