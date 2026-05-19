//! ## Declared roles
//! accessor, mapper, predicate
//!
//! Transcript path resolution through configured locators, scripts, and the
//! push-hydrated provider-private registry.

use super::errors::locator_error_to_metadata_error;
use super::locator::{
    LocatedTranscript, LocatorError, LocatorSource, SessionsConfigLocator, TranscriptLocator,
    TranscriptLookupMode, TranscriptRequest, TranscriptScriptLocator, UnsupportedStorageReason,
};
use super::{MetadataError, SessionStorageType, registry};
use oulipoly_config::{SessionSourceEntry, SessionStorage, SessionsConfig};
use std::path::PathBuf;

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
    registry.locate(
        request.provider,
        &request.session_id,
        storage_type,
        source,
        request.mode,
    )
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
