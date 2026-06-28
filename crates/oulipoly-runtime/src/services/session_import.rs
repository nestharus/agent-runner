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
    SessionProviderIdentity, SessionProviderReadTurnsRequest,
};
use chrono::{DateTime, Utc};
use oulipoly_state::ImportedSessionDisplayMetadataUpsert;
use std::collections::BTreeMap;
use std::path::Path;

const MAX_PROVIDER_SESSION_ID_BYTES: usize = 1024;
const UNKNOWN_MODEL_NAME: &str = "<unknown>";
const SESSION_ENUMERATE_CAPABILITY_MISSING: &str = "session_enumerate_capability_missing";
const SESSION_CAPABILITY_MISSING: &str = "session_capability_missing";
const SESSION_PROVIDER_DESCRIBE_UNAVAILABLE: &str = "session_provider_describe_unavailable";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EnumerateDedupKey {
    artifact_key: String,
    sessions: Vec<EnumeratedSessionSourceKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EnumeratedSessionSourceKey {
    provider_session_id: String,
    source_kind: String,
    source_detail: Option<String>,
}

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
    let mut seen_enumerations = BTreeMap::new();

    for target in request.providers {
        let provider_report =
            import_provider_sessions(&request, registry.as_ref(), target, &mut seen_enumerations);
        add_provider_report_to_totals(&mut report.totals, &provider_report);
        report.providers.push(provider_report);
    }

    Ok(SessionImportServiceOutput { report })
}

fn import_provider_sessions(
    request: &SessionImportServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
    target: &SessionImportProviderTarget,
    seen_enumerations: &mut BTreeMap<EnumerateDedupKey, String>,
) -> SessionImportProviderReport {
    let identity = target_identity(target);
    let mut report = initial_provider_report(target);
    match session_provider::enumerate_sessions(enumerate_request(
        request,
        registry,
        identity.clone(),
    )) {
        Ok(result) => {
            report.discovered = result.sessions.len() as u64;
            report.warnings.extend(result.warnings);
            if let Some(canonical_provider) = duplicate_enumeration_provider(
                registry,
                target,
                &result.sessions,
                seen_enumerations,
            ) {
                report.status = SessionImportProviderStatus::Skipped {
                    reason: format!(
                        "duplicate_enumerate_source: canonical_provider={canonical_provider}"
                    ),
                };
                report.skipped = result.sessions.len() as u64;
                return report;
            }
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

fn duplicate_enumeration_provider(
    registry: &crate::provider_registry::ProviderRegistry,
    target: &SessionImportProviderTarget,
    sessions: &[SessionProviderEnumerateEntry],
    seen_enumerations: &mut BTreeMap<EnumerateDedupKey, String>,
) -> Option<String> {
    let key = enumerate_dedup_key(registry, target, sessions);
    if let Some(canonical_provider) = seen_enumerations.get(&key) {
        return Some(canonical_provider.clone());
    }
    seen_enumerations.insert(key, target.provider_name.clone());
    None
}

fn enumerate_dedup_key(
    registry: &crate::provider_registry::ProviderRegistry,
    target: &SessionImportProviderTarget,
    sessions: &[SessionProviderEnumerateEntry],
) -> EnumerateDedupKey {
    let artifact_key = registry
        .artifact_key_for_model_provider(&target.model_name, &target.provider_name)
        .unwrap_or_else(|| {
            format!(
                "unconfigured:{}/{}",
                target.model_name, target.provider_name
            )
        });
    let mut sessions = sessions
        .iter()
        .map(enumerated_session_source_key)
        .collect::<Vec<_>>();
    sessions.sort();
    EnumerateDedupKey {
        artifact_key,
        sessions,
    }
}

fn enumerated_session_source_key(
    session: &SessionProviderEnumerateEntry,
) -> EnumeratedSessionSourceKey {
    EnumeratedSessionSourceKey {
        provider_session_id: session.provider_session_id.clone(),
        source_kind: session.source.kind.clone(),
        source_detail: session.source.detail.clone(),
    }
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
    let existed = request
        .state
        .session_chain_segment_exists_for_provider_session(
            &identity.provider_name,
            provider_session_id,
        )
        .map_err(format_import_session_state_error)?;
    if !existed {
        request
            .state
            .mint_imported_chain_if_absent(
                &identity.provider_name,
                provider_session_id,
                &provider_updated_at,
                UNKNOWN_MODEL_NAME,
            )
            .map_err(format_import_session_state_error)?;
    }
    upsert_display_metadata(
        request,
        identity,
        &entry,
        provider_session_id,
        cwd,
        provider_updated_at,
    )?;
    maybe_backfill_turns(request, registry, identity, report, provider_session_id);

    if existed {
        Ok(SessionImportEntryDisposition::Skipped)
    } else {
        Ok(SessionImportEntryDisposition::Imported)
    }
}

fn upsert_display_metadata(
    request: &SessionImportServiceRequest<'_>,
    identity: &SessionProviderIdentity,
    entry: &SessionProviderEnumerateEntry,
    provider_session_id: &str,
    cwd: Option<String>,
    provider_updated_at: DateTime<Utc>,
) -> Result<(), String> {
    let metadata = ImportedSessionDisplayMetadataUpsert {
        provider_name: identity.provider_name.clone(),
        provider_session_id: provider_session_id.to_string(),
        title: entry.title.clone(),
        cwd,
        turn_count: entry.turn_count,
        provider_updated_at: Some(provider_updated_at),
        seen_at: request.observed_at,
    };
    request
        .state
        .upsert_imported_session_display_metadata(&metadata)
        .map_err(format_import_session_state_error)
}

fn maybe_backfill_turns(
    request: &SessionImportServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
    identity: &SessionProviderIdentity,
    report: &mut SessionImportProviderReport,
    provider_session_id: &str,
) {
    if !request.backfill_turns {
        return;
    }
    match read_and_ingest_turns(request, registry, identity, provider_session_id) {
        Ok(inserted) => report.turns_backfilled += inserted,
        Err(error) => report.warnings.push(format!(
            "session.read_turns backfill failed for {provider_session_id}: {error}"
        )),
    }
}

fn read_and_ingest_turns(
    request: &SessionImportServiceRequest<'_>,
    registry: &crate::provider_registry::ProviderRegistry,
    identity: &SessionProviderIdentity,
    provider_session_id: &str,
) -> Result<u64, SessionProviderError> {
    let turns = session_provider::read_turns(SessionProviderReadTurnsRequest {
        registry,
        identity: identity.clone(),
        session_id: provider_session_id,
        effective_cwd: request.effective_cwd,
    })?;
    session_provider::ingest_owned_turns(request.state, &identity.provider_name, &turns)
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

fn target_identity(target: &SessionImportProviderTarget) -> SessionProviderIdentity {
    SessionProviderIdentity {
        model_name: target.model_name.clone(),
        provider_name: target.provider_name.clone(),
        provider_instance_id: target.provider_instance_id.clone(),
        settings_id: target.settings_id.clone(),
    }
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
