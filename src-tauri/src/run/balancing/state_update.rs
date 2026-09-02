//! Declared roles: orchestration, mapper, formatter

use oulipoly_runtime::executor;
use oulipoly_runtime::services::ProviderSessionStartMode;
use oulipoly_runtime::session_authority::{
    AuthoritativeSessionObservation, SessionAuthorityCommitRequest, SessionAuthorityExpectation,
    commit_session_authority,
};
use oulipoly_state::StateDb;
use std::path::Path;

use super::accessor::{
    BalancedExecutionEnvironment, completed_session_ingest_effective_cwd,
    session_capture_failure_reason,
};
use super::formatter;
use super::mapper::provider_session_binding;
use super::predicate::has_provider_session_id;

pub(super) fn emit_session_capture_failure(result: &executor::ExecutionResult) {
    if let Some(reason) = session_capture_failure_reason(result) {
        formatter::emit_session_capture_failure(reason);
    }
}

pub(super) fn update_session_capture(
    env: &BalancedExecutionEnvironment,
    invocation_row_id: i64,
    result: &executor::ExecutionResult,
) {
    if matches!(
        result.session_capture.method,
        executor::SessionCaptureMethod::ExternalProviderLaunch
    ) {
        return;
    }
    env.state
        .update_session_capture(
            invocation_row_id,
            result.session_capture.session_id.as_deref(),
            result.session_capture.method.db_value(),
        )
        .unwrap_or_else(formatter::emit_session_capture_update_warning);
}

pub(super) struct BalancedSessionAuthorityCommitRequest<'a> {
    pub(super) state: &'a StateDb,
    pub(super) invocation_row_id: i64,
    pub(super) invocation_uuid: &'a str,
    pub(super) expectation: SessionAuthorityExpectation<'a>,
    pub(super) observed_provider_name: &'a str,
    pub(super) start_mode: Option<ProviderSessionStartMode>,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) result: &'a executor::ExecutionResult,
}

pub(super) fn commit_balanced_session_authority(
    request: BalancedSessionAuthorityCommitRequest<'_>,
) -> Result<(), String> {
    let BalancedSessionAuthorityCommitRequest {
        state,
        invocation_row_id,
        invocation_uuid,
        expectation,
        observed_provider_name,
        start_mode,
        working_dir,
        result,
    } = request;
    let observed_session_id = match result.session_capture.method {
        executor::SessionCaptureMethod::ExternalProviderLaunch => {
            result.session_capture.session_id.as_deref()
        }
        _ => None,
    };
    commit_session_authority(SessionAuthorityCommitRequest {
        state,
        invocation_row_id,
        invocation_uuid,
        resume_input_id: matches!(start_mode, Some(ProviderSessionStartMode::Resume))
            .then(|| expectation.provider_session_id.map(str::to_string))
            .flatten(),
        expectation,
        observation: observed_session_id.map(|provider_session_id| {
            AuthoritativeSessionObservation {
                account_name: observed_provider_name,
                provider_session_id,
            }
        }),
        capture_method: result.session_capture.method.db_value(),
        provider_session_resolved_account: Some(
            completed_session_ingest_effective_cwd(working_dir)?
                .to_string_lossy()
                .into_owned(),
        ),
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

pub(super) fn bump_quota_tick(env: &BalancedExecutionEnvironment, provider_name: &str) {
    env.state
        .increment_calls_since_refresh(provider_name)
        .unwrap_or_else(formatter::emit_quota_tick_warning);
}

pub(super) fn bind_start_known_provider_session_if_present(
    state: &StateDb,
    invocation_row_id: i64,
    provider_session_id: Option<&str>,
) {
    if has_provider_session_id(provider_session_id) {
        bind_start_known_provider_session(state, invocation_row_id, provider_session_id);
    }
}

fn bind_start_known_provider_session(
    state: &StateDb,
    invocation_row_id: i64,
    provider_session_id: Option<&str>,
) {
    let Some(provider_session_id) = provider_session_id else {
        return;
    };
    state
        .bind_invocation_provider_session_start(
            invocation_row_id,
            &provider_session_binding(provider_session_id),
        )
        .unwrap_or_else(formatter::emit_provider_session_binding_warning);
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_runtime::executor::{ExecutionResult, SessionCaptureMethod, SessionCaptureResult};
    use oulipoly_state::InvocationStart;

    const INVOCATION_UUID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    #[test]
    fn endpoint_result_commits_only_the_exact_observed_session() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDb::open(&temp.path().join("state.db")).unwrap();
        let row_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "model-a".to_string(),
                provider_name: "account-a".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();

        commit_balanced_session_authority(BalancedSessionAuthorityCommitRequest {
            state: &state,
            invocation_row_id: row_id,
            invocation_uuid: INVOCATION_UUID,
            expectation: SessionAuthorityExpectation {
                account_name: "account-a",
                provider_session_id: Some("session-a"),
            },
            observed_provider_name: "account-a",
            start_mode: Some(ProviderSessionStartMode::Resume),
            working_dir: Some(temp.path()),
            result: &external_result("session-a"),
        })
        .unwrap();

        let row = state
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(row.provider_session_id.as_deref(), Some("session-a"));
        assert_eq!(row.resume_input_id.as_deref(), Some("session-a"));
        assert_eq!(
            row.provider_session_capture_method.as_deref(),
            Some("external_provider_launch")
        );
    }

    #[test]
    fn rotated_account_observation_commits_nothing_to_the_selected_account() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDb::open(&temp.path().join("state.db")).unwrap();
        let row_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "model-a".to_string(),
                provider_name: "account-a".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();

        let error = commit_balanced_session_authority(BalancedSessionAuthorityCommitRequest {
            state: &state,
            invocation_row_id: row_id,
            invocation_uuid: INVOCATION_UUID,
            expectation: SessionAuthorityExpectation {
                account_name: "account-a",
                provider_session_id: None,
            },
            observed_provider_name: "account-b",
            start_mode: None,
            working_dir: Some(temp.path()),
            result: &external_result("session-b"),
        })
        .unwrap_err();

        assert!(error.contains("session account mismatch"), "{error}");
        let row = state
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(row.provider_session_id, None);
        assert_eq!(row.provider_session_capture_method, None);
    }

    fn external_result(provider_session_id: &str) -> ExecutionResult {
        ExecutionResult {
            stdout: Vec::new(),
            stderr: String::new(),
            output_spool: None,
            exit_code: 0,
            provider_index: 0,
            session_capture: SessionCaptureResult {
                session_id: Some(provider_session_id.to_string()),
                method: SessionCaptureMethod::ExternalProviderLaunch,
            },
            resume_acceptance: None,
            terminal_reason: None,
            terminal_signal: None,
            produced_assistant_response: false,
            prompt_acceptance_attestation: None,
            captured_child_invocations: Vec::new(),
            returned_artifacts: Vec::new(),
        }
    }
}
