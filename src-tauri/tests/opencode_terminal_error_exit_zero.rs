#![cfg(unix)]

mod age153_support;

use age153_support::{Age153Fixture, assert_result_envelope_shape};
use oulipoly_state::InvocationStatus;

const INCIDENT_MODEL: &str = "age-opencode-incident";
const INCIDENT_PROVIDER: &str = "opencode";
const INCIDENT_SQLITE_ERROR_EVENT: &str = r#"{"type":"error","timestamp":1780808654364,"sessionID":"ses_15f9407ccffelCcB6CyXvpzdXK","error":{"name":"UnknownError","data":{"message":"Failed to execute statement"}}}"#;
const RECOVERED_EVENT: &str =
    r#"{"type":"assistant","message":"continued after transient provider error"}"#;
const INCIDENT_TERMINAL_REASON: &str =
    "provider error: opencode UnknownError: Failed to execute statement";

#[test]
fn opencode_terminal_error_exit_zero_finalizes_one_shot_as_failed() {
    let fixture = opencode_fixture_with_body(
        INCIDENT_MODEL,
        &opencode_body(&[INCIDENT_SQLITE_ERROR_EVENT]),
    );

    let output = fixture.run_one_shot(INCIDENT_MODEL);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = assert_result_envelope_shape(&stdout);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], -1);
    assert_eq!(result["terminal_reason"], INCIDENT_TERMINAL_REASON);
    assert_invocation_row(
        &fixture,
        InvocationStatus::Failed,
        0,
        -1,
        Some(INCIDENT_TERMINAL_REASON),
    );
}

#[test]
fn opencode_error_event_followed_by_later_event_finalizes_one_shot_as_succeeded() {
    let fixture = opencode_fixture_with_body(
        INCIDENT_MODEL,
        &opencode_body(&[INCIDENT_SQLITE_ERROR_EVENT, RECOVERED_EVENT]),
    );

    let output = fixture.run_one_shot(INCIDENT_MODEL);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = assert_result_envelope_shape(&stdout);
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["success"], true);
    assert_eq!(result["exit_code"], 0);
    assert!(result["terminal_reason"].is_null(), "{result}");
    assert_invocation_row(&fixture, InvocationStatus::Succeeded, 1, 0, None);
}

#[test]
fn opencode_terminal_error_exit_zero_finalizes_resume_as_failed() {
    let fixture = Age153Fixture::new();
    fixture.write_resume_pool(
        INCIDENT_MODEL,
        &[(
            INCIDENT_PROVIDER,
            opencode_body(&[INCIDENT_SQLITE_ERROR_EVENT]),
        )],
    );
    fixture.seed_active_chain(INCIDENT_PROVIDER, INCIDENT_MODEL);

    let output = fixture.run_resume(INCIDENT_MODEL);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_invocation_row(
        &fixture,
        InvocationStatus::Failed,
        0,
        -1,
        Some(INCIDENT_TERMINAL_REASON),
    );
}

fn opencode_fixture_with_body(model_name: &str, body: &str) -> Age153Fixture {
    let fixture = Age153Fixture::new();
    fixture.write_model(model_name, &[INCIDENT_PROVIDER]);
    fixture.write_providers_with_bodies(&[(INCIDENT_PROVIDER, body)]);
    fixture
}

fn opencode_body(lines: &[&str]) -> String {
    let mut body = lines
        .iter()
        .map(|line| format!("printf '%s\\n' '{line}'"))
        .collect::<Vec<_>>()
        .join("\n");
    body.push_str("\nexit 0");
    body
}

fn assert_invocation_row(
    fixture: &Age153Fixture,
    expected_status: InvocationStatus,
    expected_success: i64,
    expected_exit_code: i64,
    expected_terminal_reason: Option<&str>,
) {
    let (status, success, exit_code, terminal_reason): (String, i64, i64, Option<String>) = fixture
        .conn()
        .query_row(
            "SELECT status, success, exit_code, terminal_reason
             FROM invocations
             WHERE provider_name = ?1",
            [INCIDENT_PROVIDER],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();

    assert_eq!(status, expected_status.as_str());
    assert_eq!(success, expected_success);
    assert_eq!(exit_code, expected_exit_code);
    assert_eq!(terminal_reason.as_deref(), expected_terminal_reason);
}
