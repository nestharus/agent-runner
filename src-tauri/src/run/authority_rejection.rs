//! Failed-result custody is independent of permission to publish session readiness.
//! Declared roles: orchestration, mapper, formatter

use oulipoly_runtime::executor::ExecutionResult;
use oulipoly_runtime::services::{
    InvocationLifecycleFinalizeRequest, InvocationLifecycleServicePort,
};
use oulipoly_state::{ProviderLaunchOwnerFence, StateDb};

use crate::invocation::finalize::{FinalizerGuard, settle_rejected_launch};

pub(super) struct RejectedResult<'a, 'state> {
    pub service: &'a dyn InvocationLifecycleServicePort,
    pub state: &'state StateDb,
    pub invocation_row_id: i64,
    pub invocation_uuid: &'a str,
    pub guard: &'a mut FinalizerGuard<'state>,
    pub result: &'a ExecutionResult,
    pub launch_owner: Option<&'a ProviderLaunchOwnerFence>,
}

/// Never enter completion/acceptance/retry handling after authority rejection.
/// Attempt both evidence writes and failure finalization even when one fails.
/// The existing mutation scope retains allocated ownership; no standalone override.
pub(super) fn retain_and_finalize(input: RejectedResult<'_, '_>, rejection: String) -> String {
    let signal = emit_original_signal(input.result, input.invocation_uuid);
    let retained = input.result.retain_produced_evidence(
        input.state,
        input.invocation_row_id,
        input.invocation_uuid,
    );
    let reason = input
        .result
        .terminal_reason
        .as_deref()
        .unwrap_or("session_authority_rejected");
    input.guard.retain_failure(
        failure_exit_code(input.result.exit_code),
        "session_authority_rejected",
        reason,
    );
    input.guard.retain_rejected_launch(input.launch_owner);
    let finalized = input.service.finalize_invocation(
        input
            .state
            .invocation_mutation_scope(input.invocation_row_id)
            .authority(),
        InvocationLifecycleFinalizeRequest {
            state: input.state,
            invocation_row_id: input.invocation_row_id,
            success: false,
            exit_code: failure_exit_code(input.result.exit_code),
            error_category: Some("session_authority_rejected"),
            terminal_reason: Some(reason),
        },
    );
    let settlement = if finalized.is_ok() {
        input.guard.mark_finalized();
        settle_rejected_launch(input.state, input.launch_owner)
    } else {
        Ok("not attempted: invocation finalization failed")
    };
    format!(
        "session_authority_rejected: invocation={}; terminal_reason={reason}; {rejection}; evidence={}; finalization={}; terminal_signal={}; logical_settlement={}",
        input.invocation_uuid,
        retained.err().unwrap_or("retained"),
        finalized
            .map(|_| "failed invocation recorded".to_string())
            .unwrap_or_else(|error| error.to_string()),
        signal.unwrap_or_else(|error| error),
        settlement.map(str::to_string).unwrap_or_else(|error| error),
    )
}

/// Marker-only transport: deliberately do not apply terminal disposition effects.
fn emit_original_signal(result: &ExecutionResult, invocation_uuid: &str) -> Result<String, String> {
    let Some(signal) = &result.terminal_signal else {
        return Ok("absent".into());
    };
    let invocation = uuid::Uuid::parse_str(invocation_uuid).map_err(|error| error.to_string())?;
    crate::terminal_outcome_adapter::emit_terminal_signal_marker(
        signal,
        &invocation,
        None,
        &mut std::io::stderr().lock(),
    )
    .map_err(|error| format!("marker emission failed: {error}"))?;
    Ok("emitted original evidence".into())
}

fn failure_exit_code(exit_code: i32) -> i32 {
    if exit_code == 0 { -1 } else { exit_code }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_runtime::executor::{SessionCaptureMethod, SessionCaptureResult};
    use oulipoly_runtime::services::ProductionInvocationLifecycleService;
    use oulipoly_state::{InvocationStart, InvocationStatus};

    #[test]
    fn failed_storage_attempt_keeps_original_failure_for_guard_retry() {
        let root = tempfile::tempdir().unwrap();
        let state = StateDb::open(&root.path().join("state.db")).unwrap();
        let uuid = uuid::Uuid::new_v4().to_string();
        let row = state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid.clone(),
                model_name: "fixture".into(),
                provider_name: "fixture".into(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        let conn = rusqlite::Connection::open(state.path()).unwrap();
        conn.execute_batch("CREATE TRIGGER reject_finalization BEFORE UPDATE OF status ON invocations BEGIN SELECT RAISE(ABORT, 'private finalization storage fault'); END;").unwrap();
        let result = ExecutionResult {
            stdout: vec![],
            stderr: String::new(),
            output_spool: None,
            exit_code: -1,
            provider_index: 0,
            session_capture: SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::None,
            },
            resume_acceptance: None,
            terminal_reason: Some("live_session_authority_publication_failed".into()),
            terminal_signal: None,
            produced_assistant_response: false,
            prompt_acceptance_attestation: None,
            captured_child_invocations: vec![],
            returned_artifacts: vec![],
        };
        let mut guard = FinalizerGuard::new(&state, row);
        let diagnostic = retain_and_finalize(
            RejectedResult {
                service: &ProductionInvocationLifecycleService,
                state: &state,
                invocation_row_id: row,
                invocation_uuid: &uuid,
                guard: &mut guard,
                result: &result,
                launch_owner: None,
            },
            "missing authoritative observation".into(),
        );
        assert!(diagnostic.contains("live_session_authority_publication_failed"));
        assert!(diagnostic.contains("private finalization storage fault"));
        assert_eq!(
            state.get_invocation_by_id(row).unwrap().unwrap().status,
            InvocationStatus::Running
        );
        conn.execute_batch("DROP TRIGGER reject_finalization")
            .unwrap();
        drop(guard);
        assert_eq!(
            state.get_invocation_by_id(row).unwrap().unwrap().status,
            InvocationStatus::Failed
        );
        let reason: String = conn
            .query_row(
                "SELECT terminal_reason FROM invocations WHERE id=?1",
                [row],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, "live_session_authority_publication_failed");
    }
}
