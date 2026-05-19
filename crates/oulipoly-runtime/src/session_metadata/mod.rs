//! Session metadata adapter and resolver subsystem.
//!
//! ## Declared roles
//!
//! - orchestration
//! - parser
//! - formatter
//! - accessor
//! - validator
//! - predicate
//! - mapper
//! - filter
//!
//! Rationale: this module is the session-metadata adapter/resolver subsystem.
//! Focused sibling modules own transcript registry hydration, provider-private
//! locator scans, cwd resolution, and display/error mapping while this module
//! preserves the historical public API.
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/session_metadata/mod.rs::session_metadata_resolution
//!     role: intrinsic-surface
//!     Domain: session-metadata-resolution-domain
//!     Owns:
//!       - SessionMetadata, SessionStorageType, TranscriptState, and MetadataError contracts
//!       - locate_session_metadata and locate_resume_session_metadata orchestration APIs
//!       - resolve_resume_workspace_root, resolve_workspace_root_for_provider_session, and resolve_jsonl_path_for_provider APIs
//!       - UUID parser and formatter helpers
//!       - ambiguity, recency, mutability, and provider-selection predicates
//!       - ResumeError and LocatorError to MetadataError mapping

mod ambiguity;
mod cwd;
mod errors;
mod ids;
mod locator;
mod metadata_shape;
mod mutability;
mod registry;
mod resume;
#[cfg(test)]
mod tests;
mod transcript;
mod workspace;

pub use locator::{
    ClaudeStorageLocator, CodexStorageLocator, IoErrorKind, LocatedTranscript, LocatorError,
    LocatorSource, ProviderName, ScriptKind, SessionId, SessionsConfigLocator, TranscriptLocator,
    TranscriptLookupMode, TranscriptRequest, TranscriptScriptLocator, UnsupportedStorageReason,
    locator_error_to_stem, unsupported_storage_reason_to_stem,
};
pub use registry::{
    TranscriptLocatorEntry, TranscriptLocatorRegistry, discover_transcript_locator_registry,
    hydrate_transcript_locator_registry,
};

use ambiguity::{
    AmbiguityPolicy, count_recent_previews, recency_cutoff_for_resume_previews,
    reject_ambiguous_recent_matches, rejects_recent_ambiguity,
};
use errors::{map_resume_error, metadata_db_result};
use ids::{format_optional_uuid, format_uuid, parse_optional_uuid, parse_session_uuid};
use metadata_shape::{SessionMetadataParts, session_metadata_from_parts};
use mutability::is_metadata_mutable;
use oulipoly_config::{ProvidersConfig, ScriptSessionStorageType, SessionStorage, SessionsConfig};
use oulipoly_state::{ModelStore, StateDb};
use resume::{
    active_segment_id_to_metadata_error_or_value, effective_provider_for_resolved,
    fetch_active_segment_id, fetch_resume_previews,
};
use serde::Serialize;
use std::path::PathBuf;
use transcript::available_jsonl_path;
#[cfg(test)]
use transcript::locate_jsonl_path_from_storage;
pub(crate) use transcript::resolve_jsonl_path_for_provider_with_mode;
use workspace::resolve_cwd_from_session_storage;

#[derive(Debug, Clone, Serialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub chain_id: String,
    #[serde(skip)]
    pub active_segment_id: i64,
    pub provider_name: String,
    pub storage_type: SessionStorageType,
    pub jsonl_path: PathBuf,
    pub workspace_root: PathBuf,
    pub transcript_state: TranscriptState,
    pub mutable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStorageType {
    ClaudeCode,
    CodexSession,
    Other,
}

impl From<&Option<SessionStorage>> for SessionStorageType {
    fn from(storage: &Option<SessionStorage>) -> Self {
        match storage {
            Some(SessionStorage::Script { storage_type, .. }) => match storage_type {
                Some(ScriptSessionStorageType::ClaudeCode) => Self::ClaudeCode,
                Some(ScriptSessionStorageType::CodexSession) => Self::CodexSession,
                None => Self::Other,
            },
            Some(SessionStorage::ClaudeCode { .. }) => Self::ClaudeCode,
            Some(SessionStorage::Codex { .. }) => Self::CodexSession,
            None => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptState {
    Unresolved,
    NoLocator,
    Missing,
    Available,
}

impl TranscriptState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            TranscriptState::Unresolved => "unresolved",
            TranscriptState::NoLocator => "no_locator",
            TranscriptState::Missing => "missing",
            TranscriptState::Available => "available",
        }
    }
}

#[derive(Debug, Clone)]
pub enum MetadataError {
    InvalidSessionId {
        input: String,
    },
    SessionNotFound {
        input: String,
    },
    AmbiguousSession {
        input: String,
    },
    UnsupportedStorage {
        provider_name: String,
        reason: String,
    },
    Operational {
        message: String,
    },
}

pub fn locate_session_metadata(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    input: &str,
) -> Result<SessionMetadata, MetadataError> {
    locate_session_metadata_with_policy(
        state,
        models,
        providers_cfg,
        sessions_cfg,
        input,
        AmbiguityPolicy::Reject,
    )
}

pub fn locate_resume_session_metadata(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    input: &str,
) -> Result<SessionMetadata, MetadataError> {
    locate_session_metadata_with_policy(
        state,
        models,
        providers_cfg,
        sessions_cfg,
        input,
        AmbiguityPolicy::UseStrictRecency,
    )
}

fn locate_session_metadata_with_policy(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    input: &str,
    ambiguity_policy: AmbiguityPolicy,
) -> Result<SessionMetadata, MetadataError> {
    let parsed_input = parse_session_uuid(input)?;

    if rejects_recent_ambiguity(ambiguity_policy) {
        let previews = metadata_db_result(fetch_resume_previews(state, input))?;
        let cutoff = recency_cutoff_for_resume_previews();
        reject_ambiguous_recent_matches(input, count_recent_previews(&previews, cutoff))?;
    }

    let resolved = state
        .resolve_resume(models, input, None)
        .map_err(map_resume_error)?;
    let provider = effective_provider_for_resolved(&resolved, providers_cfg)?;
    let provider_name = resolved.active_provider.clone();
    let active_segment_id = active_segment_id_to_metadata_error_or_value(
        metadata_db_result(fetch_active_segment_id(state, &resolved))?,
        &resolved.chain_id,
    )?;

    let located_transcript = available_jsonl_path(
        sessions_cfg,
        provider.session_storage.as_ref(),
        &provider_name,
        &resolved.active_session_id,
    )?;
    let storage_type = located_transcript.storage_classification;
    let jsonl_path = located_transcript.path;
    let workspace_root = resolve_cwd_from_session_storage(
        provider.session_storage.as_ref(),
        &provider_name,
        &resolved.active_session_id,
    )?;

    let active_session_uuid =
        parse_optional_uuid(&resolved.active_session_id).unwrap_or(parsed_input);
    let chain_uuid = parse_optional_uuid(&resolved.chain_id);
    let mutable = is_metadata_mutable(storage_type, &provider, &jsonl_path, &workspace_root);

    Ok(session_metadata_from_parts(SessionMetadataParts {
        session_id: format_uuid(active_session_uuid),
        chain_id: format_optional_uuid(&resolved.chain_id, chain_uuid),
        active_segment_id,
        provider_name,
        storage_type,
        jsonl_path,
        workspace_root,
        mutable,
    }))
}

pub fn resolve_resume_workspace_root(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    input: &str,
) -> Result<PathBuf, MetadataError> {
    parse_session_uuid(input)?;
    let resolved = state
        .resolve_resume(models, input, None)
        .map_err(map_resume_error)?;
    let provider = effective_provider_for_resolved(&resolved, providers_cfg)?;
    resolve_cwd_from_session_storage(
        provider.session_storage.as_ref(),
        &resolved.active_provider,
        &resolved.active_session_id,
    )
}

pub fn resolve_jsonl_path_for_provider_allow_missing(
    sessions_cfg: &SessionsConfig,
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    resolve_jsonl_path_for_provider_with_mode(
        sessions_cfg,
        session_storage,
        provider_name,
        session_id,
        TranscriptLookupMode::AllowMissing,
    )
}

pub fn resolve_workspace_root_for_provider_session(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    resolve_cwd_from_session_storage(session_storage, provider_name, session_id)
}
