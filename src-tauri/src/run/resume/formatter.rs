//! Declared roles: `accessor`, `formatter`, `mapper`.

use std::fmt::Display;
use std::io::Write as _;

use oulipoly_runtime::services::{RotationFailedReason, ServiceError};
use oulipoly_state::{ResultEnvelopeFailureIdentity, StateDb};

use crate::invocation::result_envelope::emit_result_envelope;

pub(super) struct ResumeFailureOutputInput<'a> {
    pub(super) state: &'a StateDb,
    pub(super) invocation_id: &'a str,
    pub(super) provider_name: &'a str,
    pub(super) provider_session_id: &'a str,
    pub(super) exit_code: i32,
    pub(super) error_category: Option<&'a str>,
    pub(super) terminal_reason: Option<&'a str>,
    pub(super) stderr: &'a str,
}

pub(super) fn emit_stderr(message: &str) {
    eprintln!("{message}");
}

pub(super) fn emit_resume_success_output(
    invocation_id: &str,
    error_category: Option<&str>,
    result: &oulipoly_runtime::executor::ExecutionResult,
) -> std::io::Result<()> {
    if result.output_spool.is_some() {
        let mut stderr = std::io::stderr().lock();
        result.write_stderr_to(&mut stderr)?;
        stderr.flush()?;
    }
    let mut output = std::io::stdout().lock();
    result.write_stdout_to(&mut output)?;
    if result.output_spool.is_none()
        && !result.stdout_is_empty()?
        && !result.stdout_ends_with_newline()
    {
        output.write_all(b"\n")?;
    }
    output.flush()?;
    drop(output);
    if result.output_spool.is_none() {
        emit_result_envelope(
            invocation_id,
            true,
            result.exit_code,
            error_category,
            result.terminal_reason.as_deref(),
            None,
        )?;
    }
    Ok(())
}

pub(super) fn emit_resume_failure_output(input: ResumeFailureOutputInput<'_>) {
    let failure_identity = ResultEnvelopeFailureIdentity {
        agent_runner_invocation_id: input.invocation_id.to_string(),
        provider_name: Some(input.provider_name.to_string()),
        provider_session_id: Some(input.provider_session_id.to_string()),
        agent_runner_chain_id: match input
            .state
            .chain_id_for_segment(input.provider_name, input.provider_session_id)
        {
            Ok(chain_id) => chain_id,
            Err(error) => {
                emit_stderr(&format!("failed to look up chain ID: {error}"));
                None
            }
        },
    };
    if let Err(error) = emit_result_envelope(
        input.invocation_id,
        false,
        input.exit_code,
        input.error_category,
        input.terminal_reason,
        Some(&failure_identity),
    ) {
        emit_stderr(&format!("failed to deliver result envelope: {error}"));
    }
    emit_stderr(input.stderr);
    if let Some(terminal_reason) = input.terminal_reason {
        emit_stderr(terminal_reason);
    }
}

pub(super) fn emit_resume_short_line(selected_provider: &str) {
    eprintln!("[resume] -> {selected_provider}");
}

pub(super) fn emit_missing_resume_block(provider_name: &str) {
    eprintln!("provider {provider_name} has no [providers.resume] block; cannot resume");
}

pub(super) fn emit_migration_dependency_failure(message: &str) {
    eprintln!("migration failed: {message}");
}

pub(super) fn emit_migration_service_failure(error: impl Display) {
    eprintln!("migration service failed: {error}");
}

pub(super) fn emit_finalize_invocation_warning(error: impl Display) {
    eprintln!("Warning: Failed to finalize invocation: {error}");
}

pub(super) fn emit_returned_artifacts_error(error: impl Display) {
    eprintln!("Error: Failed to record returned artifacts: {error}");
}

pub(super) fn emit_routing_retry(provider_name: &str) {
    eprintln!("[routing] provider {provider_name} unavailable; rotating to another provider");
}

pub(super) fn emit_diagnostics_category(category: &str) {
    eprintln!("[diagnostics: {category}]");
}

pub(super) fn resume_provider_registry_failure(error: String) -> String {
    format!("failed to build resume provider registry: {error}")
}

pub(super) fn resume_acceptance_service_failure(error: ServiceError) -> String {
    format!("resume acceptance service failed: {error}")
}

pub(super) fn rotation_failed_reason(reason: &RotationFailedReason) -> String {
    match reason {
        RotationFailedReason::WorkingSetExhausted { candidates_tried } => format!(
            "migration failed: working set exhausted after trying providers [{}]",
            candidates_tried.join(", ")
        ),
        RotationFailedReason::ManualTargetNotInPool { target, pool } => format!(
            "cannot rotate: provider \"{target}\" is not in model pool [{}]",
            pool.join(", ")
        ),
        RotationFailedReason::ManualTargetNotMigratable { source, target } => {
            format!("cannot rotate: {source} -> {target} is not a migratable storage-class pair")
        }
        RotationFailedReason::ManualTargetIsSingleProviderPool { provider } => {
            format!("cannot rotate: model pool has only one provider ({provider})")
        }
        RotationFailedReason::ManualTargetActiveNotInPool { active } => {
            format!("cannot rotate: session-active provider \"{active}\" is not in the model pool")
        }
    }
}
