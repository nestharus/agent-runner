//! ## Declared roles
//!
//! Roles: `validator`, `orchestration`, `mapper`.
//!
//! ## Adapter declarations
//!
//! Component: `crates/oulipoly-setup/tests/setup_agent_send_turn.rs`
//! Role: adapter
//! Translates:
//! - Cargo --features __test_fixtures test invocation + CARGO_BIN_EXE_claude_stub -> deterministic stub-Command builder via set_command_builder_for_test.
//! - test-defined short deadline -> SetupAgent effective timeout via set_effective_timeout_for_test.
//! - SetupAgent::send_turn return -> deadlock-resistance / parse / extract behavior assertions (T1, T2, T4, T5, T6, T7).
//! - SetupAgent::session_id() accessor -> stderr session-id extraction assertion.

#![cfg(feature = "__test_fixtures")]

use oulipoly_setup::actions::{AgentAction, AgentTurnResult};
use oulipoly_setup::agent::SetupAgent;
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const SCHEMA: &str = r#"{"type":"object"}"#;
const MESSAGE: &str = "continue setup";
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const SEND_TURN_DEADLINE: Duration = Duration::from_secs(8);
const CHATTY_FILLER_BYTES: &str = "71680";

struct SendTurnOutcome {
    result: Result<AgentTurnResult, String>,
    session_id: Option<String>,
}

/// Risk: AGE125-T3 mirrors the existing inline constructor/session accessor
/// preservation checks from the integration crate. Selected level: Rust
/// integration test plus existing inline unit tests. Source: proposal
/// AGE125-T3.
#[test]
fn age125_t3_existing_inline_preservation() {
    let agent = SetupAgent::new("system".to_string());

    assert!(agent.session_id().is_none());
}

/// Risk: AGE125-T1 preserves normal successful `send_turn` behavior while process
/// output capture is rewritten. Selected level: Rust integration test. Source:
/// proposal AGE125-T1 and assumptions 4, 5, 6, 10, 11.
#[test]
fn age125_t1_happy_session() {
    let agent = configured_agent(stub_command(["--mode", "normal"]));

    let outcome = send_turn_with_deadline(agent, SEND_TURN_DEADLINE);

    let result = outcome.result.expect("normal stub should parse");
    assert_status_message(&result, "age125 normal status");
    assert!(result.done);
    assert_eq!(outcome.session_id.as_deref(), Some("test-session-normal"));
}

/// Risk: AGE125-T2 closes the pipe-full deadlock coverage gap for
/// `SetupAgent::send_turn`. Selected level: Rust integration test. Source:
/// proposal AGE125-T2 and assumptions 1, 2, 3, 4, 5, 6, 10, 11.
#[test]
fn age125_t2_chatty_dual_pipe_happy_session() {
    let agent = configured_agent(stub_command([
        "--mode",
        "chatty",
        "--stdout-filler-bytes",
        CHATTY_FILLER_BYTES,
        "--stderr-filler-bytes",
        CHATTY_FILLER_BYTES,
    ]));

    let outcome = send_turn_with_deadline(agent, SEND_TURN_DEADLINE);

    let result = outcome
        .result
        .expect("chatty stub should drain both pipes and parse");
    assert_status_message(&result, "age125 chatty status");
    assert!(result.done);
    assert_eq!(outcome.session_id.as_deref(), Some("test-session-chatty"));
}

/// Risk: AGE125-T4 preserves the setup-agent timeout error contract under the
/// drain rewrite. Selected level: Rust integration test. Source: proposal
/// AGE125-T4 and assumptions 2, 3, 4, 5, 9, 10, 11.
#[test]
fn age125_t4_timeout_preservation() {
    let agent = configured_agent(stub_command([
        "--mode",
        "hang",
        "--stdout-filler-bytes",
        CHATTY_FILLER_BYTES,
        "--stderr-filler-bytes",
        CHATTY_FILLER_BYTES,
    ]));

    let outcome = send_turn_with_deadline(agent, SEND_TURN_DEADLINE);

    match outcome.result {
        Ok(_) => panic!("hang stub should return a timeout error"),
        Err(err) => assert_eq!(err, "Claude CLI timed out after 120 seconds"),
    }
}

/// Risk: AGE125-T5 preserves non-zero child exit error formatting and stderr
/// capture. Selected level: Rust integration test. Source: proposal AGE125-T5
/// and assumptions 4, 5, 10, 11.
#[test]
fn age125_t5_nonzero_exit_preservation() {
    let agent = configured_agent(stub_command([
        "--mode",
        "fail",
        "--exit-code",
        "7",
        "--stderr-message",
        "age125 fail-mode stderr fragment",
    ]));

    let outcome = send_turn_with_deadline(agent, SEND_TURN_DEADLINE);
    let err = match outcome.result {
        Ok(_) => panic!("fail stub should return an error"),
        Err(err) => err,
    };

    assert!(
        err.starts_with("Claude CLI failed (exit 7): "),
        "unexpected error prefix: {err}"
    );
    assert!(
        err.contains("age125 fail-mode stderr fragment"),
        "stderr fragment missing from error: {err}"
    );
}

/// Risk: AGE125-T6 preserves stdout JSON parse-error reporting after replacing
/// process output capture. Selected level: Rust integration test. Source:
/// proposal AGE125-T6 and assumptions 4, 5, 10, 11.
#[test]
fn age125_t6_parse_error_preservation() {
    let agent = configured_agent(stub_command(["--mode", "parse-error"]));

    let outcome = send_turn_with_deadline(agent, SEND_TURN_DEADLINE);
    let err = match outcome.result {
        Ok(_) => panic!("parse-error stub should return an error"),
        Err(err) => err,
    };

    assert!(
        err.starts_with("Failed to parse agent response:"),
        "unexpected error prefix: {err}"
    );
    assert!(
        err.contains("Raw output: not-json-age125"),
        "raw invalid stdout fragment missing from error: {err}"
    );
}

/// Risk: AGE125-T7 preserves spawn-failure error formatting while other tests
/// use command-builder injection. Selected level: Rust integration test. Source:
/// proposal AGE125-T7 and assumptions 5, 9, 11.
#[test]
fn age125_t7_spawn_failure_preservation() {
    let agent = configured_agent(|| Command::new("/path/that/does/not/exist/age125-claude-stub"));

    let outcome = send_turn_with_deadline(agent, SEND_TURN_DEADLINE);
    let err = match outcome.result {
        Ok(_) => panic!("missing binary should return a spawn error"),
        Err(err) => err,
    };

    assert!(
        err.starts_with("Failed to spawn claude CLI:"),
        "unexpected error prefix: {err}"
    );
}

fn configured_agent<F>(command_builder: F) -> SetupAgent
where
    F: Fn() -> Command + Send + Sync + 'static,
{
    let mut agent = SetupAgent::new("AGE-125 setup prompt".to_string());
    agent.set_command_builder_for_test(Box::new(command_builder));
    agent.set_effective_timeout_for_test(TEST_TIMEOUT);
    agent
}

fn stub_command<const N: usize>(args: [&'static str; N]) -> impl Fn() -> Command + Send + Sync {
    move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_claude_stub"));
        command.args(args);
        command
    }
}

fn send_turn_with_deadline(agent: SetupAgent, deadline: Duration) -> SendTurnOutcome {
    let rx = spawn_send_turn_worker(agent);

    receive_outcome_with_deadline(rx, deadline)
}

fn spawn_send_turn_worker(mut agent: SetupAgent) -> Receiver<SendTurnOutcome> {
    let (tx, rx) = mpsc::channel();

    std::thread::spawn(move || {
        let result = agent.send_turn(MESSAGE, SCHEMA);
        let outcome = map_send_turn_outcome(&agent, result);
        let _ = tx.send(outcome);
    });

    rx
}

fn map_send_turn_outcome(
    agent: &SetupAgent,
    result: Result<AgentTurnResult, String>,
) -> SendTurnOutcome {
    SendTurnOutcome {
        result,
        session_id: agent.session_id().map(str::to_string),
    }
}

fn receive_outcome_with_deadline(
    rx: Receiver<SendTurnOutcome>,
    deadline: Duration,
) -> SendTurnOutcome {
    rx.recv_timeout(deadline)
        .expect("send_turn exceeded the integration-test hard deadline")
}

fn assert_status_message(result: &AgentTurnResult, expected: &str) {
    assert_eq!(result.actions.len(), 1);
    match &result.actions[0] {
        AgentAction::Status { message } => assert_eq!(message, expected),
        _ => panic!("expected a status action"),
    }
}
