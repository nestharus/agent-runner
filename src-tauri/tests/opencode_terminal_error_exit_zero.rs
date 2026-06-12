#![cfg(unix)]

mod age153_support;

use age153_support::{Age153Fixture, assert_result_envelope_shape};
use oulipoly_state::InvocationStatus;
use serde_json::Value;

const INCIDENT_MODEL: &str = "age-opencode-incident";
const INCIDENT_PROVIDER: &str = "opencode";
const INCIDENT_SQLITE_ERROR_EVENT: &str = r#"{"type":"error","timestamp":1780808654364,"sessionID":"ses_15f9407ccffelCcB6CyXvpzdXK","error":{"name":"UnknownError","data":{"message":"Failed to execute statement"}}}"#;
const RECOVERED_EVENT: &str =
    r#"{"type":"assistant","message":"continued after transient provider error"}"#;
// OpenCode's session-store SQLite contention is now a retryable signal: the
// runner rotates away from the contended provider instead of dead-ending. With
// only the contended provider in the pool there is nowhere to rotate to, so the
// run fails honestly (success=false) rather than reporting success — and the
// contended attempt is recorded as storage contention, not quota exhaustion.
const CONTENTION_REASON: &str = "provider_storage_contention";

#[test]
fn opencode_storage_contention_one_shot_rotates_then_fails_honestly() {
    let fixture = opencode_fixture_with_body(
        INCIDENT_MODEL,
        &opencode_body(&[INCIDENT_SQLITE_ERROR_EVENT]),
    );

    let output = fixture.run_one_shot(INCIDENT_MODEL);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_pool_exhausted_failure_envelope(&stdout);
    assert_contention_is_the_only_recorded_outcome(&fixture, &stdout);
}

/// After the contended attempt rotates and the single-provider pool is drained,
/// the run must surface a precise pre-invocation pool-exhausted failure envelope
/// on stdout — not a success/result envelope and not a partial/empty line.
fn assert_pool_exhausted_failure_envelope(stdout: &str) {
    assert!(
        !stdout
            .lines()
            .any(|line| line.starts_with("OULIPOLY_RESULT=")),
        "pre-invocation pool exhaustion must not emit OULIPOLY_RESULT:\n{stdout}"
    );
    let failure_lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_FAILURE="))
        .collect::<Vec<_>>();
    assert_eq!(
        failure_lines.len(),
        1,
        "expected exactly one OULIPOLY_FAILURE line:\n{stdout}"
    );
    let payload = serde_json::from_str::<Value>(failure_lines[0])
        .unwrap_or_else(|err| panic!("invalid OULIPOLY_FAILURE JSON: {err}\n{stdout}"));
    assert_eq!(payload["failure_kind"], "pre_invocation", "{payload}");
    assert_eq!(payload["stage"], "pool_exhausted", "{payload}");
    assert_eq!(payload["status"], "failed", "{payload}");
    assert_eq!(payload["success"], false, "{payload}");
    assert_eq!(
        payload["terminal_reason"], "pre_invocation_failure",
        "{payload}"
    );
    let attempted = payload["detail"]["attempted_providers"]
        .as_array()
        .unwrap_or_else(|| panic!("detail.attempted_providers must be an array: {payload}"));
    assert!(
        attempted.iter().any(|p| p == INCIDENT_PROVIDER),
        "the contended provider must appear in attempted_providers: {payload}"
    );
}

/// Every recorded attempt against the contended provider must be finalized as
/// storage contention — not mislabeled as quota exhaustion and not left as a
/// stray started/succeeded row. The contention rows must account for *all* of
/// the provider's invocation rows.
fn assert_contention_is_the_only_recorded_outcome(fixture: &Age153Fixture, context: &str) {
    let contention_rows = fixture.failed_invocation_count(INCIDENT_PROVIDER, CONTENTION_REASON);
    assert!(
        contention_rows >= 1,
        "the contended attempt must be recorded as storage contention; {context}"
    );
    assert_eq!(
        contention_rows,
        provider_invocation_count(fixture),
        "all contended attempts must be storage contention, none mislabeled or dropped; {context}"
    );
}

fn provider_invocation_count(fixture: &Age153Fixture) -> i64 {
    fixture
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM invocations WHERE provider_name = ?1",
            [INCIDENT_PROVIDER],
            |row| row.get(0),
        )
        .unwrap()
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
fn opencode_storage_contention_resume_fails_honestly() {
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
    assert_contention_is_the_only_recorded_outcome(&fixture, &format!("{output:?}"));
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

fn fetch_invocation_row(fixture: &Age153Fixture) -> (String, i64, i64, Option<String>) {
    fixture
        .conn()
        .query_row(
            "SELECT status, success, exit_code, terminal_reason
             FROM invocations
             WHERE provider_name = ?1",
            [INCIDENT_PROVIDER],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
}

fn assert_invocation_row(
    fixture: &Age153Fixture,
    expected_status: InvocationStatus,
    expected_success: i64,
    expected_exit_code: i64,
    expected_terminal_reason: Option<&str>,
) {
    let (status, success, exit_code, terminal_reason) = fetch_invocation_row(fixture);

    assert_eq!(status, expected_status.as_str());
    assert_eq!(success, expected_success);
    assert_eq!(exit_code, expected_exit_code);
    assert_eq!(terminal_reason.as_deref(), expected_terminal_reason);
}
