//! ## Declared roles
//! `accessor`, `mapper`, `predicate`, `orchestration`, `filter`
//!
//! Transcript path resolution through configured locators, scripts, and the
//! push-hydrated provider-private registry.

use super::errors::locator_error_to_metadata_error;
use super::locator::{
    LocatedTranscript, LocatorError, LocatorSource, SessionsConfigLocator, TranscriptLocator,
    TranscriptLookupMode, TranscriptRequest, TranscriptScriptLocator, UnsupportedStorageReason,
    locate_claude_by_content, locate_codex_by_content, located, single_jsonl_match,
};
use super::{MetadataError, SessionStorageType, registry};
use oulipoly_config::{SessionSourceEntry, SessionStorage, SessionsConfig};
use std::path::{Path, PathBuf};

pub(super) fn available_jsonl_path(
    sessions_cfg: &SessionsConfig,
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<LocatedTranscript, MetadataError> {
    locate_jsonl_path_with_mode(
        sessions_cfg,
        session_storage,
        provider_name,
        session_id,
        TranscriptLookupMode::RequireExisting,
    )
}

pub(crate) fn resolve_jsonl_path_for_provider_with_mode(
    sessions_cfg: &SessionsConfig,
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
    mode: TranscriptLookupMode,
) -> Result<PathBuf, MetadataError> {
    locate_jsonl_path_with_mode(
        sessions_cfg,
        session_storage,
        provider_name,
        session_id,
        mode,
    )
    .map(|located| located.path)
}

fn locate_jsonl_path_with_mode(
    sessions_cfg: &SessionsConfig,
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
    mode: TranscriptLookupMode,
) -> Result<LocatedTranscript, MetadataError> {
    let sessions_entry = sessions_cfg.get(provider_name);
    let request = transcript_request(
        provider_name,
        session_id,
        session_storage,
        sessions_entry,
        mode,
    );

    if has_sessions_transcript_locator(sessions_entry) {
        return SessionsConfigLocator
            .locate_jsonl(&request)
            .map_err(|error| locator_error_to_metadata_error(provider_name, error));
    }

    locate_jsonl_path_from_storage_request(&request)
        .map_err(|error| locator_error_to_metadata_error(provider_name, error))
}

fn transcript_request<'a>(
    provider_name: &'a str,
    session_id: &str,
    session_storage: Option<&'a SessionStorage>,
    sessions_entry: Option<&'a SessionSourceEntry>,
    mode: TranscriptLookupMode,
) -> TranscriptRequest<'a> {
    TranscriptRequest {
        provider: provider_name,
        session_id: session_id.to_string(),
        storage: session_storage,
        sessions_config_locator: sessions_entry,
        mode,
    }
}

fn has_sessions_transcript_locator(sessions_entry: Option<&SessionSourceEntry>) -> bool {
    sessions_entry
        .and_then(|entry| entry.transcript_locator.as_ref())
        .is_some()
}

fn locate_jsonl_path_from_storage_request(
    request: &TranscriptRequest,
) -> Result<LocatedTranscript, LocatorError> {
    match request.storage {
        Some(SessionStorage::Script { .. }) => TranscriptScriptLocator.locate_jsonl(request),
        Some(SessionStorage::ClaudeCode { .. }) => locate_jsonl_path_from_registry(
            request,
            SessionStorageType::ClaudeCode,
            LocatorSource::Claude,
        ),
        Some(SessionStorage::Codex { .. }) => locate_jsonl_path_from_registry(
            request,
            SessionStorageType::CodexSession,
            LocatorSource::Codex,
        ),
        None => Err(no_locator_for_unknown_storage_error()),
    }
}

fn locate_jsonl_path_from_registry(
    request: &TranscriptRequest,
    storage_type: SessionStorageType,
    source: LocatorSource,
) -> Result<LocatedTranscript, LocatorError> {
    let registry = registry::registry_for_provider_storage(request.provider, request.storage)?;
    let registry_result = registry.locate(
        request.provider,
        &request.session_id,
        storage_type,
        source,
        request.mode,
    );
    if !is_registry_not_found(&registry_result, source) {
        return registry_result;
    }
    locate_via_content_fallback(request, storage_type, source)
}

fn is_registry_not_found(
    result: &Result<LocatedTranscript, LocatorError>,
    source: LocatorSource,
) -> bool {
    matches!(
        result,
        Err(LocatorError::NotFound { source: error_source, .. }) if *error_source == source
    )
}

// AGE-157: parity with `scripts/claude-code-locate-transcript` and
// `scripts/codex-locate-transcript`, which fall back to scanning JSONL
// content for the session id when filename-derived lookup misses.
fn locate_via_content_fallback(
    request: &TranscriptRequest,
    storage_type: SessionStorageType,
    source: LocatorSource,
) -> Result<LocatedTranscript, LocatorError> {
    let matches = content_fallback_matches(request.storage, &request.session_id, source)?;
    let path = single_jsonl_match(source, &request.session_id, matches)?;
    Ok(located(path, storage_type, request.mode))
}

fn content_fallback_matches(
    storage: Option<&SessionStorage>,
    session_id: &str,
    source: LocatorSource,
) -> Result<Vec<PathBuf>, LocatorError> {
    match (storage, source) {
        (Some(SessionStorage::ClaudeCode { projects_dir }), LocatorSource::Claude) => {
            content_fallback_dir(projects_dir)
                .and_then(|dir| locate_claude_by_content(&dir, session_id))
        }
        (Some(SessionStorage::Codex { sessions_dir }), LocatorSource::Codex) => {
            content_fallback_dir(sessions_dir)
                .and_then(|dir| locate_codex_by_content(&dir, session_id))
        }
        _ => Ok(Vec::new()),
    }
}

fn content_fallback_dir(dir: &Path) -> Result<PathBuf, LocatorError> {
    dir.canonicalize().map_err(|error| LocatorError::Io {
        kind: error.kind(),
        path: dir.to_path_buf(),
        message: format!(
            "transcript_content_fallback_dir_unavailable: {}: {error}",
            dir.display()
        ),
    })
}

fn no_locator_for_unknown_storage_error() -> LocatorError {
    LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::NoLocatorForUnknownStorage,
    }
}

#[cfg(test)]
pub(super) fn locate_jsonl_path_from_storage(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
    require_existing_file: bool,
) -> Result<PathBuf, MetadataError> {
    let request = transcript_request_from_storage(
        session_storage,
        provider_name,
        session_id,
        mode_from_require_existing(require_existing_file),
    );
    locate_jsonl_path_from_storage_request(&request)
        .map(|located| located.path)
        .map_err(|error| locator_error_to_metadata_error(provider_name, error))
}

#[cfg(test)]
fn mode_from_require_existing(require_existing_file: bool) -> TranscriptLookupMode {
    if require_existing_file {
        TranscriptLookupMode::RequireExisting
    } else {
        TranscriptLookupMode::AllowMissing
    }
}

#[cfg(test)]
fn transcript_request_from_storage<'a>(
    session_storage: Option<&'a SessionStorage>,
    provider_name: &'a str,
    session_id: &str,
    mode: TranscriptLookupMode,
) -> TranscriptRequest<'a> {
    transcript_request(provider_name, session_id, session_storage, None, mode)
}
