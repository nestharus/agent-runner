//! ## Declared roles
//!
//! `orchestration`, `mapper`, `accessor`

use std::collections::HashMap;
use std::path::Path;

use oulipoly_config::ModelConfig;
use oulipoly_runtime::executor;
use oulipoly_runtime::services::InvocationLifecycleServicePort;
use oulipoly_state::CompositeInvocationId;
use oulipoly_state::mailbox::{MailboxDb, SessionRuntimeUpsert};

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

const TERMINAL_PERSISTENCE_ERROR_CATEGORY: &str = "terminal_persistence";
const TERMINAL_PERSISTENCE_TERMINAL_REASON: &str = "terminal_persistence_failed";

pub(super) struct CompletedAttemptInput<'a, 'state, 'ctx> {
    pub(super) agent_runtime_services: &'a wiring::AgentRuntimeServices,
    pub(super) env: &'a BalancedExecutionEnvironment,
    pub(super) invocation: &'a CompositeInvocationId,
    pub(super) invocation_row_id: i64,
    pub(super) guard: &'a mut FinalizerGuard<'state>,
    pub(super) provider_name: &'a str,
    pub(super) model: &'a ModelConfig,
    pub(super) provider_index: usize,
    pub(super) result: &'a executor::ExecutionResult,
    pub(super) terminal_signal: &'a Option<executor::TerminalSignal>,
    pub(super) terminal_signal_ctx: &'a mut TerminalSignalContext<'ctx, std::io::Stderr>,
    pub(super) all_models: &'a HashMap<String, ModelConfig>,
    pub(super) working_dir: Option<&'a Path>,
    pub(super) zero_turn_provider_session_id: Option<&'a str>,
    pub(super) attempts: usize,
    pub(super) max_attempts: usize,
    pub(super) recovered_generic_nonzero: bool,
}

pub(super) fn finalize_completed_attempt(
    mut input: CompletedAttemptInput<'_, '_, '_>,
) -> BalancedLoopControl {
    apply_terminal_signal_outcome_for_completed_attempt(&mut input);
    supervise_captured_child_invocations(
        &input.env.state,
        input.invocation_row_id,
        &input.result.captured_child_invocations,
        input.result.terminal_reason.as_deref(),
    );

    emit_session_capture_failure(input.result);
    update_session_capture(input.env, input.invocation_row_id, input.result);

    let classification = classify_completed_attempt_with_balanced_result_error_category(&input);

    if let Some(control) = handle_returned_artifacts_persist_failure(&mut input) {
        return control;
    }

    formatter::emit_unknown_diagnostic_if_settled_unknown(
        &input.env.state,
        input.provider_name,
        input.provider_index,
        input.result,
        settled_unknown(classification.error_category.as_deref()),
        "no_retry",
    );

    if let Some(control) = finalize_completed_attempt_lifecycle(&mut input, &classification) {
        return control;
    }

    if classification.success
        && let Err(err) = ingest_completed_attempt_session(&input)
    {
        mark_balanced_attempt_idle(&input, Some(input.result.exit_code));
        return BalancedLoopControl::Return(Err(err));
    }

    bump_quota_tick(input.env, input.provider_name);

    completed_attempt_result_control(&input, &classification)
}

fn apply_terminal_signal_outcome_for_completed_attempt(
    input: &mut CompletedAttemptInput<'_, '_, '_>,
) {
    if input.recovered_generic_nonzero {
        emit_recovered_terminal_signal_marker(input);
        return;
    }
    let terminal_signal_disposition =
        apply_terminal_signal_outcome(input.terminal_signal, input.terminal_signal_ctx);
    validator::expect_completed_attempt_disposition(terminal_signal_disposition);
}

fn emit_recovered_terminal_signal_marker(input: &mut CompletedAttemptInput<'_, '_, '_>) {
    let Some(signal) = input.terminal_signal else {
        return;
    };
    if let Err(err) = crate::terminal_outcome_adapter::emit_terminal_signal_marker(
        signal,
        input.terminal_signal_ctx.invocation_id,
        input.terminal_signal_ctx.session_id,
        &mut input.terminal_signal_ctx.stderr,
    ) {
        tracing::warn!("Failed to emit recovered terminal signal marker: {err}");
    }
}

struct CompletedAttemptClassification {
    success: bool,
    error_category: Option<String>,
    quota_exhausted: bool,
}

fn classify_completed_attempt_with_balanced_result_error_category(
    input: &CompletedAttemptInput<'_, '_, '_>,
) -> CompletedAttemptClassification {
    let success = input.recovered_generic_nonzero || execution_succeeded(input.result);
    let error_category = (!input.recovered_generic_nonzero)
        .then(|| {
            balanced_result_error_category(
                input.agent_runtime_services,
                input.result,
                input.all_models,
                input.working_dir,
            )
        })
        .flatten();
    let quota_exhausted = error_category_is_quota_exhausted(error_category.as_deref());
    CompletedAttemptClassification {
        success,
        error_category,
        quota_exhausted,
    }
}

fn handle_returned_artifacts_persist_failure(
    input: &mut CompletedAttemptInput<'_, '_, '_>,
) -> Option<BalancedLoopControl> {
    let Err(err) = record_returned_artifacts_for_completed_attempt(input) else {
        return None;
    };
    finalize_returned_artifacts_persist_failure(mapper::artifact_persist_failure_input(
        mapper::ArtifactPersistFailureInputSource {
            agent_runtime_services: input.agent_runtime_services,
            env: input.env,
            invocation: input.invocation,
            invocation_row_id: input.invocation_row_id,
            guard: &mut *input.guard,
            provider_name: input.provider_name,
            provider_session_id: input.zero_turn_provider_session_id,
            error: &err,
        },
    ));
    Some(BalancedLoopControl::Return(Ok(1)))
}

fn finalize_completed_attempt_lifecycle(
    input: &mut CompletedAttemptInput<'_, '_, '_>,
    classification: &CompletedAttemptClassification,
) -> Option<BalancedLoopControl> {
    let finalize_result = input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::completed_finalize_request(
            &input.env.state,
            input.invocation_row_id,
            classification.success,
            input.result.exit_code,
            classification.error_category.as_deref(),
            input.result.terminal_reason.as_deref(),
        ));
    if let Err(err) = finalize_result {
        emit_completed_attempt_finalize_failure(input, err);
        return Some(BalancedLoopControl::Return(Ok(1)));
    }
    input.guard.mark_finalized();
    None
}

fn emit_completed_attempt_finalize_failure(
    input: &CompletedAttemptInput<'_, '_, '_>,
    err: impl std::fmt::Display,
) {
    formatter::emit_finalize_invocation_warning(err);
    formatter::emit_failure_result_envelope(mapper::failure_result_envelope_input(
        &input.env.state,
        &input.invocation.id,
        input.provider_name,
        input.zero_turn_provider_session_id,
        1,
        Some(TERMINAL_PERSISTENCE_ERROR_CATEGORY),
        Some(TERMINAL_PERSISTENCE_TERMINAL_REASON),
    ));
    mark_balanced_attempt_idle(input, Some(1));
}

fn completed_attempt_result_control(
    input: &CompletedAttemptInput<'_, '_, '_>,
    classification: &CompletedAttemptClassification,
) -> BalancedLoopControl {
    if classification.success {
        return emit_completed_attempt_success(input, classification);
    }
    if let Some(control) = quota_exhausted_retry_control(input, classification) {
        return control;
    }
    emit_completed_attempt_failure(input, classification)
}

fn emit_completed_attempt_success(
    input: &CompletedAttemptInput<'_, '_, '_>,
    classification: &CompletedAttemptClassification,
) -> BalancedLoopControl {
    mark_balanced_successful_attempt_idle_and_recheck(input, input.result.exit_code);
    formatter::emit_success_output(
        &input.invocation.id,
        input.result.exit_code,
        classification.error_category.as_deref(),
        input.result.terminal_reason.as_deref(),
        &input.result.stdout,
    );
    BalancedLoopControl::Return(Ok(if input.recovered_generic_nonzero {
        0
    } else {
        input.result.exit_code
    }))
}

fn quota_exhausted_retry_control(
    input: &CompletedAttemptInput<'_, '_, '_>,
    classification: &CompletedAttemptClassification,
) -> Option<BalancedLoopControl> {
    if !classification.quota_exhausted {
        return None;
    }
    mark_balanced_attempt_idle(input, Some(input.result.exit_code));
    if retry_available(input.attempts, input.max_attempts) {
        formatter::emit_routing_retry(input.provider_name);
    }
    Some(BalancedLoopControl::Continue)
}

fn emit_completed_attempt_failure(
    input: &CompletedAttemptInput<'_, '_, '_>,
    classification: &CompletedAttemptClassification,
) -> BalancedLoopControl {
    formatter::emit_failure_output_with_diagnostics(
        mapper::completed_attempt_failure_result_envelope_input(
            &input.env.state,
            &input.invocation.id,
            input.provider_name,
            input.zero_turn_provider_session_id,
            input.result,
            classification.error_category.as_deref(),
        ),
        &input.result.stderr,
        classification.error_category.as_deref(),
    );
    mark_balanced_attempt_idle(input, Some(input.result.exit_code));
    BalancedLoopControl::Return(Ok(input.result.exit_code))
}

fn mark_balanced_attempt_idle(input: &CompletedAttemptInput<'_, '_, '_>, exit_code: Option<i32>) {
    if finalize_balanced_pty_handoff(
        input.zero_turn_provider_session_id,
        &input.invocation.id,
        exit_code,
    ) {
        return;
    }
    let Some(provider_session_id) = input.zero_turn_provider_session_id else {
        return;
    };
    if let Err(err) = crate::wake_coordinator::mark_session_idle_after_turn(
        provider_session_id,
        &input.invocation.id,
        exit_code,
    ) {
        tracing::warn!(
            session_id = provider_session_id,
            invocation_uuid = input.invocation.id,
            "Failed to mark balanced session idle: {err}"
        );
    }
}

fn mark_balanced_successful_attempt_idle_and_recheck(
    input: &CompletedAttemptInput<'_, '_, '_>,
    exit_code: i32,
) {
    if finalize_balanced_pty_handoff(
        input.zero_turn_provider_session_id,
        &input.invocation.id,
        Some(exit_code),
    ) {
        return;
    }
    let Some(provider_session_id) = input.zero_turn_provider_session_id else {
        return;
    };
    if let Err(err) = crate::wake_coordinator::mark_successful_turn_idle_and_recheck(
        provider_session_id,
        &input.invocation.id,
        exit_code,
    ) {
        tracing::warn!(
            session_id = provider_session_id,
            invocation_uuid = input.invocation.id,
            "Failed to run balanced wake recheck: {err}"
        );
    }
}

fn finalize_balanced_pty_handoff(
    provider_session_id: Option<&str>,
    invocation_uuid: &str,
    exit_code: Option<i32>,
) -> bool {
    let Some(exit_code) = exit_code else {
        return false;
    };
    match crate::mailbox_delivery::finalize_pty_mailbox_delivery_handoff(
        provider_session_id,
        invocation_uuid,
        exit_code,
    ) {
        Ok(handled) => handled,
        Err(err) => {
            tracing::warn!(
                session_id = provider_session_id,
                invocation_uuid,
                "Failed to hand off balanced PTY mailbox delivery: {err}"
            );
            false
        }
    }
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
    emit_returned_artifacts_persist_failure(&input);
    match finalize_returned_artifacts_failure_lifecycle(&input) {
        Ok(_) => input.guard.mark_finalized(),
        Err(err) => formatter::emit_finalize_invocation_warning(err),
    }
    if !finalize_balanced_pty_handoff(input.provider_session_id, input.invocation_id, Some(1)) {
        let Some(provider_session_id) = input.provider_session_id else {
            return;
        };
        if let Err(err) = crate::wake_coordinator::mark_session_idle_after_turn(
            provider_session_id,
            input.invocation_id,
            Some(1),
        ) {
            tracing::warn!(
                session_id = provider_session_id,
                invocation_uuid = input.invocation_id,
                "Failed to mark balanced session idle: {err}"
            );
        }
    }
}

fn emit_returned_artifacts_persist_failure(input: &mapper::ArtifactPersistFailureInput<'_, '_>) {
    formatter::emit_returned_artifacts_error(input.error);
    formatter::emit_failure_result_envelope(mapper::failure_result_envelope_input(
        &input.env.state,
        input.invocation_id,
        input.provider_name,
        input.provider_session_id,
        1,
        Some("returned_artifacts"),
        Some("returned_artifacts_persist_failed"),
    ));
}

fn finalize_returned_artifacts_failure_lifecycle(
    input: &mapper::ArtifactPersistFailureInput<'_, '_>,
) -> Result<(), oulipoly_runtime::services::ServiceError> {
    input
        .agent_runtime_services
        .invocation_lifecycle_service
        .finalize_invocation(mapper::returned_artifacts_finalize_request(
            &input.env.state,
            input.invocation_row_id,
        ))
        .map(|_| ())
}

fn ingest_completed_attempt_session(
    input: &CompletedAttemptInput<'_, '_, '_>,
) -> Result<(), String> {
    let ingest_effective_cwd =
        super::accessor::completed_session_ingest_effective_cwd(input.working_dir)?;
    let ingest_output = ingest_and_emit_session_id_resume_aware(
        input.agent_runtime_services,
        mapper::completed_session_ingest_request_for_attempt(input, &ingest_effective_cwd),
    );
    record_external_session_runtime_if_needed(
        input,
        &ingest_effective_cwd,
        ingest_output.session_id.as_deref(),
    );
    if let Some(session_id) = super::filter::session_ingest_fallback_session_id(
        ingest_output.emitted,
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

fn record_external_session_runtime_if_needed(
    input: &CompletedAttemptInput<'_, '_, '_>,
    effective_cwd: &Path,
    ingested_session_id: Option<&str>,
) {
    if input.model.provider.is_none() {
        return;
    }
    let Some(session_id) = external_runtime_session_id(input, ingested_session_id) else {
        return;
    };
    if let Err(err) = record_external_session_runtime(input, session_id, effective_cwd) {
        tracing::warn!(
            provider_name = input.provider_name,
            session_id,
            "Failed to record external session runtime cwd: {err}"
        );
    }
}

fn external_runtime_session_id<'a>(
    input: &'a CompletedAttemptInput<'_, '_, '_>,
    ingested_session_id: Option<&'a str>,
) -> Option<&'a str> {
    input
        .zero_turn_provider_session_id
        .or(ingested_session_id)
        .or(input.result.session_capture.session_id.as_deref())
}

fn record_external_session_runtime(
    input: &CompletedAttemptInput<'_, '_, '_>,
    session_id: &str,
    effective_cwd: &Path,
) -> Result<(), String> {
    let mut db = MailboxDb::open_default()?;
    let effective_cwd = effective_cwd.to_string_lossy();
    db.upsert_session_runtime(SessionRuntimeUpsert {
        session_id,
        mode: "headless",
        invocation_uuid: Some(&input.invocation.id),
        provider_name: Some(input.provider_name),
        model_name: Some(&input.model.name),
        pty_control_path: None,
        models_dir: Some(input.env.models_dir.to_string_lossy().as_ref()),
        effective_cwd: Some(effective_cwd.as_ref()),
        selected_auto_wake_max: Some(crate::wake_coordinator::selected_auto_wake_max()),
    })
}
