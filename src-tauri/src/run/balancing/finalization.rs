//! orchestration

use std::collections::HashMap;
use std::path::Path;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_state::CompositeInvocationId;

use super::accessor::BalancedExecutionEnvironment;
use super::disposition::BalancedLoopControl;
use super::formatter;
use super::mapper;
use super::predicate::{
    error_category_is_quota_exhausted, execution_succeeded, retry_available, settled_unknown,
};
use super::state_update::{bump_quota_tick, emit_session_capture_failure, update_session_capture};
use super::validator;
use crate::captured_child::supervise_captured_child_invocations;
use crate::invocation::finalize::FinalizerGuard;
use crate::quota_zero_turn::balanced_result_error_category;
use crate::session_ingest_cli::{emit_known_session_id, ingest_and_emit_session_id_resume_aware};
use crate::terminal_outcome_adapter::{TerminalSignalContext, apply_terminal_signal_outcome};
use crate::wiring;

pub(super) struct CompletedAttemptInput<'a, 'state, 'ctx> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) provider_index: usize,
    pub(super) result: &'a executor::ExecutionResult,
    pub(super) terminal_signal: &'a Option<executor::TerminalSignal>,
    pub(super) terminal_signal_ctx: &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    pub(super) all_models: &'a HashMap<String, ModelConfig>,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) zero_turn_provider_session_id: Option<&'a str>,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
}

pub(super) fn finalize_completed_attempt(
    input: CompletedAttemptInput<'_, '_, '_>,
) -> BalancedLoopControl {
    let terminal_signal_disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_completed_attempt_disposition(terminal_signal_disposition);
    supervise_captured_child_invocations(
        &input.env.state,
        input.invocation_row_id,
        &input.result.captured_child_invocations,
        input.result.terminal_reason.as_deref(),
    );

    emit_session_capture_failure(input.result);
    update_session_capture(input.env, input.invocation_row_id, input.result);

    let success = execution_succeeded(input.result);

    let error_category = balanced_result_error_category(
        input.agent_runtime_services,
        input.result,
        input.all_models,
        input.working_dir,
    );
    let quota_exhausted = error_category_is_quota_exhausted(error_category.as_deref());

    if let Err(err) = record_returned_artifacts_for_completed_attempt(&input) {
        finalize_returned_artifacts_persist_failure(mapper::artifact_persist_failure_input(
            mapper::ArtifactPersistFailureInputSource {
                agent_runtime_services: input.agent_runtime_services,
                env: input.env,
                invocation: input.invocation,
                invocation_row_id: input.invocation_row_id,
                guard: input.guard,
                provider_name: input.provider_name,
                provider_session_id: input.zero_turn_provider_session_id,
                error: &err,
            },
        ));
        return BalancedLoopControl::Return(Ok(1));
    }

    formatter::emit_unknown_diagnostic_if_settled_unknown(
        &input.env.state,
        input.provider_name,
        input.provider_index,
        input.result,
        settled_unknown(error_category.as_deref()),
        "no_retry",
    );

    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::completed_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            success,
            input.result.exit_code,
            error_category.as_deref(),
            input.result.terminal_reason.as_deref(),
        ))
        .map(|_| ())
        .unwrap_or_else(formatter::emit_finalize_invocation_warning);
    input.guard.mark_finalized();

    if success && let Err(err) = ingest_completed_attempt_session(&input) {
        return BalancedLoopControl::Return(Err(err));
    }

    bump_quota_tick(input.env, input.provider_name);

    if success {
        formatter::emit_success_output(
            &input.invocation.id,
            input.result.exit_code,
            error_category.as_deref(),
            input.result.terminal_reason.as_deref(),
            &input.result.stdout,
        );
        return BalancedLoopControl::Return(Ok(input.result.exit_code));
    }
    if quota_exhausted {
        if retry_available(input.attempts, input.max_attempts) {
            formatter::emit_routing_retry(input.provider_name);
        }
        return BalancedLoopControl::Continue;
    }

    formatter::emit_failure_output_with_diagnostics(
        mapper::completed_attempt_failure_result_envelope_input(
            &input.env.state,
            &input.invocation.id,
            input.provider_name,
            input.zero_turn_provider_session_id,
            input.result,
            error_category.as_deref(),
        ),
        &input.result.stderr,
        error_category.as_deref(),
    );
    BalancedLoopControl::Return(Ok(input.result.exit_code))
}

fn record_returned_artifacts_for_completed_attempt(
    input: &CompletedAttemptInput<'_, '_, '_>,
) -> Result<(), String> {
    super::accessor::record_returned_artifacts(
        &input.env.state,
        input.invocation_row_id,
        &input.result.returned_artifacts,
    )
}

fn finalize_returned_artifacts_persist_failure(input: mapper::ArtifactPersistFailureInput<'_, '_>) {
    formatter::emit_returned_artifacts_error(input.error);
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::returned_artifacts_finalize_request(
            &input.env.state,
            input.invocation_row_id,
        ))
        .map(|_| ())
        .unwrap_or_else(formatter::emit_finalize_invocation_warning);
    formatter::emit_failure_result_envelope(mapper::failure_result_envelope_input(
        &input.env.state,
        input.invocation_id,
        input.provider_name,
        input.provider_session_id,
        1,
        Some("returned_artifacts"),
        Some("returned_artifacts_persist_failed"),
    ));
    input.guard.mark_finalized();
}

fn ingest_completed_attempt_session(
    input: &CompletedAttemptInput<'_, '_, '_>,
) -> Result<(), String> {
    let ingest_effective_cwd =
        super::accessor::completed_session_ingest_effective_cwd(input.working_dir)?;
    let emitted = ingest_and_emit_session_id_resume_aware(
        input.agent_runtime_services,
        mapper::completed_session_ingest_request_for_attempt(input, &ingest_effective_cwd),
    );
    if let Some(session_id) = super::filter::session_ingest_fallback_session_id(
        emitted,
        input.result.session_capture.session_id.as_deref(),
    ) {
        emit_known_session_id(
            &input.env.state,
            input.invocation_row_id,
            &input.invocation.id,
            session_id,
            input.result.session_capture.method.db_value(),
        );
    }
    Ok(())
}
