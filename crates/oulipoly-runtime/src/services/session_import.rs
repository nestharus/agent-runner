//! Provider-native session import service.

use super::dtos::{
    SessionImportProviderReport, SessionImportProviderStatus, SessionImportProviderTarget,
    SessionImportReport, SessionImportServiceOutput, SessionImportServiceRequest,
    SessionImportTotals,
};
use super::error::ServiceError;
use crate::provider_registry::ProviderRegistryHandle;
use crate::session_provider::{
    self, SessionProviderEnumerateEntry, SessionProviderEnumerateRequest, SessionProviderError,
    SessionProviderIdentity, SessionTurnIngestDriverRequest, SessionTurnIngestQuantumOutcome,
    run_session_turn_ingest_quantum_for_key,
};
use chrono::{DateTime, Utc};
use oulipoly_provider::client::CancellationToken;
use oulipoly_state::{ImportedSessionDisplayMetadataUpsert, SessionTurnIngestStreamKey};
use std::path::Path;

const MAX_PROVIDER_SESSION_ID_BYTES: usize = 1024;
const UNKNOWN_MODEL_NAME: &str = "<unknown>";
const SESSION_ENUMERATE_CAPABILITY_MISSING: &str = "session_enumerate_capability_missing";
const SESSION_CAPABILITY_MISSING: &str = "session_capability_missing";
const SESSION_PROVIDER_DESCRIBE_UNAVAILABLE: &str = "session_provider_describe_unavailable";
const MAX_SYNCHRONOUS_BACKFILL_PAGES: usize = 4096;

pub(super) fn import_sessions_with_registry(
    request: SessionImportServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<SessionImportServiceOutput, ServiceError> {
    let registry = provider_registry
        .ok_or_else(session_import_registry_unavailable)?
        .current();
    let mut report = SessionImportReport {
        providers: Vec::with_capacity(request.providers.len()),
        totals: SessionImportTotals {
            providers_total: request.providers.len() as u64,
            ..SessionImportTotals::default()
        },
    };
    for target in request.providers {
        let provider_report = import_provider_sessions(&request, registry.as_ref(), target);
        add_provider_report_to_totals(&mut report.totals, &provider_report);
        report.providers.push(provider_report);
    }

    Ok(SessionImportServiceOutput { report })
}

fn import_provider_sessions(
    request: &SessionImportServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
    target: &SessionImportProviderTarget,
) -> SessionImportProviderReport {
    let mut report = initial_provider_report(target);
    let identity = match target_identity(registry, target) {
        Ok(identity) => identity,
        Err(error) => {
            if missing_enumerate_capability(&error) {
                report.status = SessionImportProviderStatus::Skipped {
                    reason: error.to_string(),
                };
            } else {
                report.errors.push(error.to_string());
            }
            return report;
        }
    };
    match session_provider::enumerate_sessions(enumerate_request(
        request,
        registry,
        identity.clone(),
    )) {
        Ok(result) => {
            report.discovered = result.sessions.len() as u64;
            report.warnings.extend(result.warnings);
            import_enumerated_entries(request, registry, &identity, &mut report, result.sessions);
            report.status = SessionImportProviderStatus::Succeeded;
        }
        Err(error) if missing_enumerate_capability(&error) => {
            report.status = SessionImportProviderStatus::Skipped {
                reason: error.to_string(),
            };
        }
        Err(error) => {
            report.status = SessionImportProviderStatus::Failed;
            report.errors.push(error.to_string());
        }
    }
    report
}

fn enumerate_request<'a>(
    request: &'a SessionImportServiceRequest<'a>,
    registry: &'a crate::provider_registry::ProviderRegistry,
    identity: SessionProviderIdentity,
) -> SessionProviderEnumerateRequest<'a> {
    SessionProviderEnumerateRequest {
        registry,
        identity,
        limit: request.limit,
        cursor: None,
        include_cwd: true,
        include_turn_count: true,
        since_unix_ms: request.since_unix_ms,
        effective_cwd: request.effective_cwd,
    }
}

fn import_enumerated_entries(
    request: &SessionImportServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
    identity: &SessionProviderIdentity,
    report: &mut SessionImportProviderReport,
    sessions: Vec<SessionProviderEnumerateEntry>,
) {
    for entry in sessions {
        match import_enumerated_entry(request, registry, identity, report, entry) {
            Ok(SessionImportEntryDisposition::Imported) => report.imported += 1,
            Ok(SessionImportEntryDisposition::Skipped) => report.skipped += 1,
            Err(message) => report.errors.push(message),
        }
    }
}

enum SessionImportEntryDisposition {
    Imported,
    Skipped,
}

fn import_enumerated_entry(
    request: &SessionImportServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
    identity: &SessionProviderIdentity,
    report: &mut SessionImportProviderReport,
    entry: SessionProviderEnumerateEntry,
) -> Result<SessionImportEntryDisposition, String> {
    let provider_session_id = validate_provider_session_id(&entry.provider_session_id)?;
    let provider_updated_at = normalize_entry_timestamp(&entry, request.observed_at, report)?;
    let cwd = normalize_cwd(entry.cwd.as_deref())?;
    let metadata = imported_display_metadata(
        request,
        identity,
        &entry,
        provider_session_id,
        cwd,
        provider_updated_at,
    );
    let stream = session_provider::canonical_stream_key(identity, provider_session_id)
        .map_err(|error| error.to_string())?;
    let imported = request
        .state
        .import_session_and_enqueue_turn_ingest(
            &metadata,
            &stream,
            &provider_updated_at,
            UNKNOWN_MODEL_NAME,
        )
        .map_err(format_import_session_state_error)?;
    if request.backfill_turns {
        backfill_enumerated_entry(request, registry, report, &stream);
    }

    if imported {
        Ok(SessionImportEntryDisposition::Imported)
    } else {
        Ok(SessionImportEntryDisposition::Skipped)
    }
}

fn backfill_enumerated_entry(
    request: &SessionImportServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
    report: &mut SessionImportProviderReport,
    stream: &SessionTurnIngestStreamKey,
) {
    let cancellation = CancellationToken::new();
    let lease_owner = format!(
        "session-import-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    for _ in 0..MAX_SYNCHRONOUS_BACKFILL_PAGES {
        let outcome = run_session_turn_ingest_quantum_for_key(
            SessionTurnIngestDriverRequest {
                state: request.state,
                registry,
                lease_owner: &lease_owner,
                effective_cwd: request.effective_cwd,
                cancellation: &cancellation,
                now: Utc::now(),
            },
            stream,
        );
        match outcome {
            Ok(SessionTurnIngestQuantumOutcome::Applied { inserted_turns, .. }) => {
                report.turns_backfilled = report.turns_backfilled.saturating_add(inserted_turns);
                if request
                    .state
                    .session_turn_ingest_stream(stream)
                    .ok()
                    .flatten()
                    .is_some_and(|stream| stream.status == "caught_up")
                {
                    return;
                }
            }
            Ok(SessionTurnIngestQuantumOutcome::RetryScheduled { error, .. }) => {
                report.warnings.push(format!(
                    "canonical turn backfill is retryable for {}: {error}",
                    stream.session_id
                ));
                return;
            }
            Ok(SessionTurnIngestQuantumOutcome::Unsupported { error, .. }) => {
                report.warnings.push(format!(
                    "canonical turn backfill is unsupported for {}: {error}",
                    stream.session_id
                ));
                return;
            }
            Ok(SessionTurnIngestQuantumOutcome::Quarantined { error, .. }) => {
                report.errors.push(format!(
                    "canonical turn backfill quarantined for {}: {error}",
                    stream.session_id
                ));
                return;
            }
            Ok(SessionTurnIngestQuantumOutcome::Idle) => {
                let status = request
                    .state
                    .session_turn_ingest_stream(stream)
                    .ok()
                    .flatten()
                    .map(|stream| stream.status)
                    .unwrap_or_else(|| "missing".to_string());
                if status != "caught_up" {
                    report.warnings.push(format!(
                        "canonical turn backfill is retryable for {}: stream status {status}",
                        stream.session_id
                    ));
                }
                return;
            }
            Err(error) => {
                report.errors.push(format!(
                    "canonical turn backfill failed for {}: {error}",
                    stream.session_id
                ));
                return;
            }
        }
    }
    report.warnings.push(format!(
        "canonical turn backfill is retryable for {}: synchronous page budget exhausted",
        stream.session_id
    ));
}

fn imported_display_metadata(
    request: &SessionImportServiceRequest<'_>,
    identity: &SessionProviderIdentity,
    entry: &SessionProviderEnumerateEntry,
    provider_session_id: &str,
    cwd: Option<String>,
    provider_updated_at: DateTime<Utc>,
) -> ImportedSessionDisplayMetadataUpsert {
    ImportedSessionDisplayMetadataUpsert {
        provider_name: identity.provider_name.clone(),
        provider_session_id: provider_session_id.to_string(),
        title: entry.title.clone(),
        cwd,
        turn_count: entry.turn_count,
        provider_updated_at: Some(provider_updated_at),
        seen_at: request.observed_at,
    }
}

fn validate_provider_session_id(provider_session_id: &str) -> Result<&str, String> {
    if provider_session_id.trim().is_empty() {
        return Err("provider returned an empty session id".to_string());
    }
    if provider_session_id.len() > MAX_PROVIDER_SESSION_ID_BYTES {
        return Err(format!(
            "provider session id exceeds {MAX_PROVIDER_SESSION_ID_BYTES} bytes"
        ));
    }
    Ok(provider_session_id)
}

fn normalize_entry_timestamp(
    entry: &SessionProviderEnumerateEntry,
    observed_at: DateTime<Utc>,
    report: &mut SessionImportProviderReport,
) -> Result<DateTime<Utc>, String> {
    if let Some(updated_unix_ms) = entry.updated_unix_ms {
        return unix_ms_to_utc(updated_unix_ms);
    }
    if let Some(created_unix_ms) = entry.created_unix_ms {
        return unix_ms_to_utc(created_unix_ms);
    }
    report.warnings.push(format!(
        "session {} missing provider timestamps; used import observed_at",
        entry.provider_session_id
    ));
    Ok(observed_at)
}

fn unix_ms_to_utc(unix_ms: u64) -> Result<DateTime<Utc>, String> {
    let unix_ms = i64::try_from(unix_ms)
        .map_err(|_| "provider session timestamp exceeds supported range".to_string())?;
    DateTime::<Utc>::from_timestamp_millis(unix_ms)
        .ok_or_else(|| "provider session timestamp is out of range".to_string())
}

fn normalize_cwd(cwd: Option<&Path>) -> Result<Option<String>, String> {
    cwd.map(|path| {
        if path.is_absolute() {
            Ok(path.to_string_lossy().into_owned())
        } else {
            Err("provider returned a relative session cwd".to_string())
        }
    })
    .transpose()
}

fn target_identity(
    registry: &crate::provider_registry::ProviderRegistry,
    target: &SessionImportProviderTarget,
) -> Result<SessionProviderIdentity, SessionProviderError> {
    let endpoint = registry
        .preflight_account(&target.provider_name)
        .map_err(|error| {
            SessionProviderError::new(SESSION_PROVIDER_DESCRIBE_UNAVAILABLE, error.to_string())
        })?;
    let settings_id = endpoint.settings_id().map_err(|error| {
        SessionProviderError::new(SESSION_PROVIDER_DESCRIBE_UNAVAILABLE, error.to_string())
    })?;
    if settings_id != target.settings_id {
        return Err(SessionProviderError::new(
            "session_provider_identity_mismatch",
            "session import settings identity does not match the selected account endpoint",
        ));
    }
    Ok(SessionProviderIdentity {
        model_name: target.model_name.clone(),
        provider_name: target.provider_name.clone(),
        provider_instance_id: Some(format!("{}-instance", endpoint.capabilities().provider_id)),
        settings_id: settings_id.to_string(),
    })
}

fn initial_provider_report(target: &SessionImportProviderTarget) -> SessionImportProviderReport {
    SessionImportProviderReport {
        model_name: target.model_name.clone(),
        provider_name: target.provider_name.clone(),
        settings_id: target.settings_id.clone(),
        status: SessionImportProviderStatus::Failed,
        discovered: 0,
        imported: 0,
        skipped: 0,
        errors: Vec::new(),
        warnings: Vec::new(),
        turns_backfilled: 0,
    }
}

fn missing_enumerate_capability(error: &SessionProviderError) -> bool {
    matches!(
        error.token(),
        SESSION_ENUMERATE_CAPABILITY_MISSING
            | SESSION_CAPABILITY_MISSING
            | SESSION_PROVIDER_DESCRIBE_UNAVAILABLE
    )
}

fn add_provider_report_to_totals(
    totals: &mut SessionImportTotals,
    provider: &SessionImportProviderReport,
) {
    match provider.status {
        SessionImportProviderStatus::Succeeded => totals.providers_succeeded += 1,
        SessionImportProviderStatus::Skipped { .. } => totals.providers_skipped += 1,
        SessionImportProviderStatus::Failed => totals.providers_failed += 1,
    }
    totals.discovered += provider.discovered;
    totals.imported += provider.imported;
    totals.skipped += provider.skipped;
    totals.errors += provider.errors.len() as u64;
    totals.warnings += provider.warnings.len() as u64;
    totals.turns_backfilled += provider.turns_backfilled;
}

fn session_import_registry_unavailable() -> ServiceError {
    ServiceError::Unavailable {
        message: "session_import_registry_unavailable".to_string(),
        code: Some("session_import_registry_unavailable".to_string()),
    }
}

fn format_import_session_state_error(error: String) -> String {
    format!("state import failed: {error}")
}
