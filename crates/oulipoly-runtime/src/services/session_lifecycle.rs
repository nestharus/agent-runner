//! ## Declared roles
//! orchestration, accessor, validator, mapper, predicate, formatter
//!
//! Session lifecycle orchestration helper. It sequences invocation lookup,
//! provider scanning, session-window selection, and marker emission.

use super::dtos::{SessionLifecycleIngestMode, SessionLifecycleOutput, SessionLifecycleRequest};
use super::error::ServiceError;
use super::marker::{emit_known_session_id_for_service, emit_pinned_session_id_for_service};
use super::session_warning::write_session_ingest_warning;
use super::session_window::find_session_for_invocation_window;
use oulipoly_state::{InvocationRecord, StateDb};
use std::io::Write;

pub(super) fn ingest_session(
    request: SessionLifecycleRequest<'_>,
) -> Result<SessionLifecycleOutput, ServiceError> {
    let SessionLifecycleRequest {
        state,
        sessions_cfg,
        providers_cfg,
        provider_name,
        invocation_row_id,
        invocation_uuid,
        effective_cwd,
        mode,
        stderr,
    } = request;

    let Some(invocation) =
        load_or_warn_for_session_ingest(state, stderr, invocation_row_id, invocation_uuid, &mode)?
    else {
        return emit_pinned_session_id_for_service(
            state,
            stderr,
            invocation_row_id,
            invocation_uuid,
            mode,
        );
    };
    validate_session_ingest_invocation(&invocation, invocation_row_id, invocation_uuid)?;
    let Some(finished_at) = invocation_finished_at(&invocation) else {
        write_unfinalized_invocation_warning(stderr, invocation_uuid)?;
        return emit_pinned_session_id_for_service(
            state,
            stderr,
            invocation_row_id,
            invocation_uuid,
            mode,
        );
    };

    let report = crate::sessions::scan_provider(provider_name, sessions_cfg, state);
    emit_session_scan_errors(stderr, provider_name, report.errors)?;
    let matched_session_id = resolve_session_window_match(SessionWindowMatchRequest {
        state,
        providers_cfg,
        provider_name,
        invocation: &invocation,
        finished_at: &finished_at,
        effective_cwd,
        stderr,
        invocation_uuid,
    })?;

    emit_session_lifecycle_output(
        state,
        stderr,
        invocation_row_id,
        invocation_uuid,
        &invocation,
        matched_session_id,
        mode,
    )
}

fn load_or_warn_for_session_ingest(
    state: &StateDb,
    stderr: &mut dyn Write,
    _invocation_row_id: i64,
    invocation_uuid: &str,
    _mode: &SessionLifecycleIngestMode,
) -> Result<Option<InvocationRecord>, ServiceError> {
    match load_invocation_for_session_ingest(state, invocation_uuid) {
        Ok(Some(row)) => Ok(Some(row)),
        Ok(None) => {
            write_missing_invocation_warning(stderr, invocation_uuid)?;
            Ok(None)
        }
        Err(err) => {
            write_invocation_load_failure_warning(stderr, invocation_uuid, err)?;
            Ok(None)
        }
    }
}

struct SessionWindowMatchRequest<'a> {
    state: &'a StateDb,
    providers_cfg: Option<&'a oulipoly_config::ProvidersConfig>,
    provider_name: &'a str,
    invocation: &'a InvocationRecord,
    finished_at: &'a chrono::DateTime<chrono::Utc>,
    effective_cwd: Option<&'a std::path::Path>,
    stderr: &'a mut dyn Write,
    invocation_uuid: &'a str,
}

fn resolve_session_window_match(
    request: SessionWindowMatchRequest<'_>,
) -> Result<Option<String>, ServiceError> {
    match find_session_for_invocation_window(
        request.state,
        request.providers_cfg,
        request.provider_name,
        &request.invocation.created_at,
        request.finished_at,
        request.effective_cwd,
        request.stderr,
    ) {
        Ok(session_id) => Ok(session_id),
        Err(err) => {
            write_session_window_match_failure_warning(
                request.stderr,
                request.invocation_uuid,
                err,
            )?;
            Ok(None)
        }
    }
}

fn load_invocation_for_session_ingest(
    state: &StateDb,
    invocation_uuid: &str,
) -> Result<Option<InvocationRecord>, String> {
    state.get_invocation_by_uuid(invocation_uuid)
}

fn validate_session_ingest_invocation(
    invocation: &InvocationRecord,
    invocation_row_id: i64,
    invocation_uuid: &str,
) -> Result<(), ServiceError> {
    if invocation.id != invocation_row_id {
        return Err(mismatched_session_ingest_invocation_error(
            invocation_row_id,
            invocation_uuid,
        ));
    }
    Ok(())
}

fn invocation_finished_at(invocation: &InvocationRecord) -> Option<chrono::DateTime<chrono::Utc>> {
    invocation.finished_at
}

fn emit_session_scan_errors(
    stderr: &mut dyn Write,
    provider_name: &str,
    errors: Vec<String>,
) -> Result<(), ServiceError> {
    for err in errors {
        write_session_ingest_warning(
            stderr,
            &format!("Session ingest failed for {provider_name}: {err}"),
        )?;
    }
    Ok(())
}

fn emit_session_lifecycle_output(
    state: &StateDb,
    stderr: &mut dyn Write,
    invocation_row_id: i64,
    invocation_uuid: &str,
    invocation: &InvocationRecord,
    matched_session_id: Option<String>,
    mode: SessionLifecycleIngestMode,
) -> Result<SessionLifecycleOutput, ServiceError> {
    match mode {
        SessionLifecycleIngestMode::Unpinned { capture_method } => emit_unpinned_session_output(
            state,
            stderr,
            invocation_row_id,
            invocation_uuid,
            invocation,
            matched_session_id,
            &capture_method,
        ),
        SessionLifecycleIngestMode::Pinned { resume_target } => emit_known_session_id_for_service(
            state,
            stderr,
            invocation_row_id,
            invocation_uuid,
            &resume_target,
            "resumed",
        ),
    }
}

fn emit_unpinned_session_output(
    state: &StateDb,
    stderr: &mut dyn Write,
    invocation_row_id: i64,
    invocation_uuid: &str,
    invocation: &InvocationRecord,
    matched_session_id: Option<String>,
    capture_method: &str,
) -> Result<SessionLifecycleOutput, ServiceError> {
    match select_unpinned_session_source(invocation, matched_session_id, capture_method) {
        UnpinnedSessionSource::StartBound {
            session_id,
            capture_method,
            matched_session_id,
        } => {
            warn_if_preserving_start_bound_session(stderr, &matched_session_id, session_id)?;
            emit_known_session_id_for_service(
                state,
                stderr,
                invocation_row_id,
                invocation_uuid,
                session_id,
                capture_method,
            )
        }
        UnpinnedSessionSource::Matched(session_id) => emit_known_session_id_for_service(
            state,
            stderr,
            invocation_row_id,
            invocation_uuid,
            &session_id,
            capture_method,
        ),
        UnpinnedSessionSource::None => Ok(empty_session_lifecycle_output()),
    }
}

enum UnpinnedSessionSource<'a> {
    StartBound {
        session_id: &'a str,
        capture_method: &'a str,
        matched_session_id: Option<String>,
    },
    Matched(String),
    None,
}

fn select_unpinned_session_source<'a>(
    invocation: &'a InvocationRecord,
    matched_session_id: Option<String>,
    capture_method: &'a str,
) -> UnpinnedSessionSource<'a> {
    if let Some(session_id) = invocation.provider_session_id.as_deref() {
        return UnpinnedSessionSource::StartBound {
            session_id,
            capture_method: invocation
                .provider_session_capture_method
                .as_deref()
                .unwrap_or(capture_method),
            matched_session_id,
        };
    }
    matched_session_source(matched_session_id)
}

fn warn_if_preserving_start_bound_session(
    stderr: &mut dyn Write,
    matched_session_id: &Option<String>,
    session_id: &str,
) -> Result<(), ServiceError> {
    if preserves_different_start_bound_session(matched_session_id, session_id) {
        write_preserving_start_bound_session_warning(stderr, matched_session_id, session_id)?;
    }
    Ok(())
}

fn write_unfinalized_invocation_warning(
    stderr: &mut dyn Write,
    invocation_uuid: &str,
) -> Result<(), ServiceError> {
    write_session_ingest_warning(
        stderr,
        &format!("Invocation {invocation_uuid} was not finalized before session ingest"),
    )
}

fn write_missing_invocation_warning(
    stderr: &mut dyn Write,
    invocation_uuid: &str,
) -> Result<(), ServiceError> {
    write_session_ingest_warning(
        stderr,
        &format!("Could not resolve invocation {invocation_uuid} for session ingest"),
    )
}

fn write_invocation_load_failure_warning(
    stderr: &mut dyn Write,
    invocation_uuid: &str,
    err: String,
) -> Result<(), ServiceError> {
    write_session_ingest_warning(
        stderr,
        &format!("Failed to load invocation {invocation_uuid} for session ingest: {err}"),
    )
}

fn write_session_window_match_failure_warning(
    stderr: &mut dyn Write,
    invocation_uuid: &str,
    err: String,
) -> Result<(), ServiceError> {
    write_session_ingest_warning(
        stderr,
        &format!("Failed to resolve session for invocation {invocation_uuid}: {err}"),
    )
}

fn mismatched_session_ingest_invocation_error(
    invocation_row_id: i64,
    invocation_uuid: &str,
) -> ServiceError {
    ServiceError::InvalidRequest {
        message: format!(
            "session ingest request mismatched invocation identifiers: row_id={invocation_row_id} uuid={invocation_uuid}"
        ),
    }
}

fn matched_session_source(matched_session_id: Option<String>) -> UnpinnedSessionSource<'static> {
    matched_session_id
        .map(UnpinnedSessionSource::Matched)
        .unwrap_or(UnpinnedSessionSource::None)
}

fn empty_session_lifecycle_output() -> SessionLifecycleOutput {
    SessionLifecycleOutput {
        emitted: false,
        session_id: None,
    }
}

fn preserves_different_start_bound_session(
    matched_session_id: &Option<String>,
    session_id: &str,
) -> bool {
    matched_session_id
        .as_deref()
        .is_some_and(|matched| matched != session_id)
}

fn write_preserving_start_bound_session_warning(
    stderr: &mut dyn Write,
    matched_session_id: &Option<String>,
    session_id: &str,
) -> Result<(), ServiceError> {
    write_session_ingest_warning(
        stderr,
        &format!(
            "post-run session inference found {matched:?}, preserving start-bound provider_session_id {session_id}",
            matched = matched_session_id.as_deref()
        ),
    )
}
