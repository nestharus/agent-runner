//! mapper

use oulipoly_config::{ModelConfig, PromptMode};
use oulipoly_runtime::executor;
use oulipoly_runtime::services::{
    InvocationLifecycleFinalizeRequest, InvocationLifecycleStartRequest,
};
use oulipoly_state::{CompositeInvocationId, InvocationStart, ProviderSessionBinding, StateDb};
use uuid::Uuid;

use crate::session_ingest_cli::ResumeIngestMode;

pub(super) fn provider_default_model() -> ModelConfig {
    ModelConfig {
        name: "<provider-default>".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: Vec::new(),
        inputs: Vec::new(),
        provider: None,
    }
}

pub(super) fn composite_invocation_id(provider_name: &str) -> CompositeInvocationId {
    CompositeInvocationId {
        source: provider_name.to_string(),
        id: Uuid::new_v4().to_string(),
    }
}

pub(super) fn invocation_start(
    invocation: &CompositeInvocationId,
    model_name: String,
    provider_name: &str,
    provider_index: usize,
    parent_invocation_id: Option<i64>,
) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation.id.clone(),
        model_name,
        provider_name: provider_name.to_string(),
        provider_index,
        parent_invocation_id,
    }
}

pub(super) fn invocation_lifecycle_start_request<'a>(
    state: &'a StateDb,
    start: &'a InvocationStart,
) -> InvocationLifecycleStartRequest<'a> {
    InvocationLifecycleStartRequest { state, start }
}

pub(super) fn resumed_provider_session_binding(
    provider_session_id: &str,
    resume_input_id: Option<String>,
) -> ProviderSessionBinding {
    ProviderSessionBinding {
        provider_session_id: provider_session_id.to_string(),
        capture_method: "resumed",
        resume_input_id,
        provider_session_resolved_account: None,
    }
}

pub(super) fn resume_payload<'a>(
    provider: &'a oulipoly_config::ProviderConfig,
    session_id: &'a str,
) -> executor::cli::ResumePayload<'a> {
    let strategy = provider
        .resume
        .as_ref()
        .expect("resumable provider must have a resume strategy");
    executor::cli::ResumePayload {
        session_id,
        strategy,
    }
}

pub(super) fn finalize_request<'a>(
    state: &'a StateDb,
    invocation_row_id: i64,
    success: bool,
    exit_code: i32,
    error_category: Option<&'a str>,
    terminal_reason: Option<&'a str>,
) -> InvocationLifecycleFinalizeRequest<'a> {
    InvocationLifecycleFinalizeRequest {
        state,
        invocation_row_id,
        success,
        exit_code,
        error_category,
        terminal_reason,
    }
}

pub(super) fn spawn_error_finalize_request(
    state: &StateDb,
    invocation_row_id: i64,
) -> InvocationLifecycleFinalizeRequest<'_> {
    finalize_request(
        state,
        invocation_row_id,
        false,
        1,
        Some("spawn_error"),
        Some("spawn_error"),
    )
}

pub(super) fn repl_ingest_mode<'a>(
    resume: Option<&'a str>,
    manual_migrate: Option<&str>,
    resume_session_id: Option<&'a str>,
) -> ResumeIngestMode<'a> {
    match resume {
        Some(session_id) => ResumeIngestMode::Pinned {
            resume_target: if manual_migrate.is_some() {
                session_id
            } else {
                resume_session_id.unwrap_or(session_id)
            },
        },
        None => ResumeIngestMode::Unpinned {
            capture_method: "turn_script",
        },
    }
}

pub(super) struct TerminalSignalContextIds {
    pub(super) invocation_uuid: Uuid,
    pub(super) session_uuid: Option<Uuid>,
}

pub(super) fn terminal_signal_context_ids(
    invocation_id: &str,
    provider_session_id: Option<&str>,
) -> TerminalSignalContextIds {
    TerminalSignalContextIds {
        invocation_uuid: Uuid::parse_str(invocation_id)
            .expect("generated invocation id must be a UUID"),
        session_uuid: crate::dispatch::provider_session_marker_uuid(provider_session_id),
    }
}

pub(super) fn terminal_signal_context_for_repl<'a, W: std::io::Write>(
    ids: &'a TerminalSignalContextIds,
    provider_name: &'a str,
    state: &'a StateDb,
    stderr: &'a mut W,
) -> crate::terminal_outcome_adapter::TerminalSignalContext<'a, W> {
    crate::terminal_outcome_adapter::TerminalSignalContext {
        invocation_id: &ids.invocation_uuid,
        session_id: ids.session_uuid.as_ref(),
        provider: provider_name,
        state_db: state,
        stderr,
    }
}
