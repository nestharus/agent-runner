mod cwd;
mod locator;
#[cfg(test)]
mod tests;

pub use locator::{
    ClaudeStorageLocator, CodexStorageLocator, IoErrorKind, LocatedTranscript, LocatorError,
    LocatorSource, ProviderName, ScriptKind, SessionId, SessionsConfigLocator, TranscriptLocator,
    TranscriptLookupMode, TranscriptRequest, TranscriptScriptLocator, UnsupportedStorageReason,
    locator_error_to_stem, unsupported_storage_reason_to_stem,
};

use oulipoly_config::{
    ModelConfig, ProviderConfig, ProvidersConfig, ScriptSessionStorageType, SessionStorage,
    SessionsConfig,
};
use oulipoly_state::{ModelStore, ResumeError, StateDb};
use serde::Serialize;
use std::path::PathBuf;
use uuid::Uuid;

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

#[derive(Debug, Clone, Copy)]
enum AmbiguityPolicy {
    Reject,
    UseStrictRecency,
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

fn parse_session_uuid(input: &str) -> Result<Uuid, MetadataError> {
    parse_uuid(input).map_err(|_| invalid_session_id_error(input))
}

fn parse_uuid(input: &str) -> Result<Uuid, uuid::Error> {
    Uuid::parse_str(input)
}

fn parse_optional_uuid(input: &str) -> Option<Uuid> {
    parse_uuid(input).ok()
}

fn format_uuid(uuid: Uuid) -> String {
    uuid.to_string()
}

fn format_optional_uuid(fallback: &str, parsed: Option<Uuid>) -> String {
    parsed
        .map(format_uuid)
        .unwrap_or_else(|| fallback.to_string())
}

fn invalid_session_id_error(input: &str) -> MetadataError {
    MetadataError::InvalidSessionId {
        input: input.to_string(),
    }
}

fn metadata_db_result<T>(result: Result<T, String>) -> Result<T, MetadataError> {
    result.map_err(operational_error)
}

fn fetch_resume_previews(
    state: &StateDb,
    input: &str,
) -> Result<Vec<oulipoly_state::ChainPreview>, String> {
    state.resume_previews(input)
}

fn fetch_active_segment_id(
    state: &StateDb,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<Option<i64>, String> {
    state.active_segment_id_for_chain_provider_session(
        &resolved.chain_id,
        &resolved.active_provider,
        &resolved.active_session_id,
    )
}

fn active_segment_id_to_metadata_error_or_value(
    active_segment_id: Option<i64>,
    chain_id: &str,
) -> Result<i64, MetadataError> {
    active_segment_id.ok_or_else(|| session_not_found_error(chain_id))
}

fn session_not_found_error(input: &str) -> MetadataError {
    MetadataError::SessionNotFound {
        input: input.to_string(),
    }
}

fn rejects_recent_ambiguity(policy: AmbiguityPolicy) -> bool {
    matches!(policy, AmbiguityPolicy::Reject)
}

fn recency_cutoff_for_resume_previews() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::hours(24)
}

fn count_recent_previews(
    previews: &[oulipoly_state::ChainPreview],
    cutoff: chrono::DateTime<chrono::Utc>,
) -> usize {
    previews
        .iter()
        .filter(|preview| preview.last_used_at >= cutoff)
        .count()
}

fn reject_ambiguous_recent_matches(input: &str, recent_count: usize) -> Result<(), MetadataError> {
    if recent_count > 1 {
        return Err(MetadataError::AmbiguousSession {
            input: input.to_string(),
        });
    }
    Ok(())
}

struct SessionMetadataParts {
    session_id: String,
    chain_id: String,
    active_segment_id: i64,
    provider_name: String,
    storage_type: SessionStorageType,
    jsonl_path: PathBuf,
    workspace_root: PathBuf,
    mutable: bool,
}

fn is_metadata_mutable(
    storage_type: SessionStorageType,
    provider: &ProviderConfig,
    jsonl_path: &std::path::Path,
    workspace_root: &std::path::Path,
) -> bool {
    storage_type != SessionStorageType::Other
        && provider.resume.is_some()
        && jsonl_path.is_absolute()
        && workspace_root.is_absolute()
}

fn session_metadata_from_parts(parts: SessionMetadataParts) -> SessionMetadata {
    SessionMetadata {
        session_id: parts.session_id,
        chain_id: parts.chain_id,
        active_segment_id: parts.active_segment_id,
        provider_name: parts.provider_name,
        storage_type: parts.storage_type,
        jsonl_path: parts.jsonl_path,
        workspace_root: parts.workspace_root,
        transcript_state: TranscriptState::Available,
        mutable: parts.mutable,
    }
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

fn map_resume_error(err: ResumeError) -> MetadataError {
    match err {
        ResumeError::InvalidUuid { input } => MetadataError::InvalidSessionId { input },
        ResumeError::NoChainFound { input } => MetadataError::SessionNotFound { input },
        ResumeError::WrongIdKind { input, .. } => MetadataError::SessionNotFound { input },
        ResumeError::Ambiguous { input, .. } => MetadataError::AmbiguousSession { input },
        ResumeError::UnknownModel { model_name } => {
            operational_error(unknown_model_message(&model_name))
        }
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            ..
        } => operational_error(provider_model_mismatch_message(
            &model_name,
            &active_provider,
        )),
        ResumeError::ProviderNotConfigured { provider } => MetadataError::UnsupportedStorage {
            provider_name: provider.clone(),
            reason: provider_not_configured_reason(&provider),
        },
        ResumeError::ActiveSegmentMissing { chain_id } => {
            MetadataError::SessionNotFound { input: chain_id }
        }
        ResumeError::ProviderMissingResume { provider_name } => MetadataError::UnsupportedStorage {
            provider_name: provider_name.clone(),
            reason: provider_missing_resume_reason(&provider_name),
        },
        ResumeError::Db { message } => MetadataError::Operational { message },
    }
}

fn operational_error(message: String) -> MetadataError {
    MetadataError::Operational { message }
}

fn unknown_model_message(model_name: &str) -> String {
    format!("unknown model {model_name}")
}

fn provider_model_mismatch_message(model_name: &str, active_provider: &str) -> String {
    format!("model {model_name} does not include active provider {active_provider}")
}

fn provider_not_configured_reason(provider: &str) -> String {
    format!("provider {provider} is not configured")
}

fn provider_missing_resume_reason(provider_name: &str) -> String {
    format!("provider {provider_name} has no resume strategy")
}

fn effective_provider_for_resolved(
    resolved: &oulipoly_state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ProviderConfig, MetadataError> {
    if let Some(model) = resolved.model.as_ref() {
        let model_provider = active_model_provider(model, &resolved.active_provider)
            .ok_or_else(|| provider_mismatch_error(model, &resolved.active_provider))?;
        effective_model_provider(providers_cfg, model_provider, &resolved.active_provider)
    } else {
        runtime_provider(providers_cfg, &resolved.active_provider)
    }
}

fn active_model_provider<'a>(
    model: &'a ModelConfig,
    active_provider: &str,
) -> Option<&'a ProviderConfig> {
    model
        .providers
        .iter()
        .find(|provider| provider.name == active_provider)
}

fn provider_mismatch_error(model: &ModelConfig, active_provider: &str) -> MetadataError {
    operational_error(provider_model_mismatch_message(
        &model.name,
        active_provider,
    ))
}

fn effective_model_provider(
    providers_cfg: &ProvidersConfig,
    model_provider: &ProviderConfig,
    active_provider: &str,
) -> Result<ProviderConfig, MetadataError> {
    let (provider, _) = providers_cfg
        .effective_provider(model_provider)
        .map_err(|message| provider_resolution_error(active_provider, message))?;
    Ok(provider)
}

fn runtime_provider(
    providers_cfg: &ProvidersConfig,
    active_provider: &str,
) -> Result<ProviderConfig, MetadataError> {
    let (provider, _) = providers_cfg
        .runtime_provider(active_provider)
        .map_err(|message| provider_resolution_error(active_provider, message))?;
    Ok(provider)
}

fn provider_resolution_error(provider_name: &str, reason: String) -> MetadataError {
    MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason,
    }
}

fn available_jsonl_path(
    sessions_cfg: &SessionsConfig,
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<LocatedTranscript, MetadataError> {
    let mode = TranscriptLookupMode::RequireExisting;
    locate_jsonl_path_with_mode(
        sessions_cfg,
        session_storage,
        provider_name,
        session_id,
        mode,
    )
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
    sessions_entry: Option<&'a oulipoly_config::SessionSourceEntry>,
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

fn has_sessions_transcript_locator(
    sessions_entry: Option<&oulipoly_config::SessionSourceEntry>,
) -> bool {
    sessions_entry
        .and_then(|entry| entry.transcript_locator.as_ref())
        .is_some()
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

fn locator_error_to_metadata_error(provider_name: &str, error: LocatorError) -> MetadataError {
    MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason: locator_error_to_stem(error),
    }
}

fn locate_jsonl_path_from_storage_request(
    request: &TranscriptRequest,
) -> Result<LocatedTranscript, LocatorError> {
    match request.storage {
        Some(SessionStorage::Script { .. }) => TranscriptScriptLocator.locate_jsonl(request),
        Some(SessionStorage::ClaudeCode { .. }) => ClaudeStorageLocator.locate_jsonl(request),
        Some(SessionStorage::Codex { .. }) => CodexStorageLocator.locate_jsonl(request),
        None => Err(no_locator_for_unknown_storage_error()),
    }
}

fn no_locator_for_unknown_storage_error() -> LocatorError {
    LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::NoLocatorForUnknownStorage,
    }
}

#[cfg(test)]
fn locate_jsonl_path_from_storage(
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

fn resolve_cwd_from_session_storage(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    cwd::resolve_workspace_root(session_storage, session_id)
        .map_err(|reason| unsupported_storage_error(provider_name, reason))
}

pub fn resolve_workspace_root_for_provider_session(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    resolve_cwd_from_session_storage(session_storage, provider_name, session_id)
}

fn unsupported_storage_error(provider_name: &str, reason: String) -> MetadataError {
    MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason,
    }
}
