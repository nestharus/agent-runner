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
//! preserves the historical public API through those runner-owned interfaces.
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
//!       - TranscriptLocatorRegistry and TranscriptLocator common-interface
//!         handoff for provider-private transcript storage
//!       - session-resolution inputs StateDb, ModelStore, ResolvedResume, and the
//!         oulipoly-config session/provider/storage types (ProvidersConfig,
//!         SessionsConfig, ModelConfig, SessionStorage, ScriptSessionStorageType)
//!         that resume/metadata resolution reads
//!       - external-provider session-locate surface (ProviderRegistry,
//!         session_provider identity/locate/describe, oulipoly_provider DescribeResult)
//!       - session-runtime workspace-root lookup over
//!         oulipoly_state::mailbox::MailboxDb session_runtime rows
//!       - script-backed imported-session workspace-root fallback through
//!         invocations.provider_session_resolved_account
//!       - the session-metadata-resolution sibling resolvers (ambiguity, cwd,
//!         errors, ids, locator, metadata_shape, mutability, registry, resume,
//!         transcript, workspace) this facade sequences

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

use crate::provider_registry::ProviderRegistry;
use crate::session_provider::{self, SessionProviderIdentity, SessionProviderLocateRequest};
use ambiguity::{
    AmbiguityPolicy, count_recent_previews, recency_cutoff_for_resume_previews,
    reject_ambiguous_recent_matches, rejects_recent_ambiguity,
};
use errors::{map_resume_error, metadata_db_result};
use ids::{format_optional_uuid, format_uuid, parse_optional_uuid, parse_session_uuid};
use metadata_shape::{SessionMetadataParts, session_metadata_from_parts};
use mutability::is_metadata_mutable;
use oulipoly_config::{
    ModelConfig, ProvidersConfig, ScriptSessionStorageType, SessionStorage, SessionsConfig,
};
use oulipoly_state::{ModelStore, ResolvedResume, StateDb};
use resume::{
    active_segment_id_to_metadata_error_or_value, effective_provider_for_resolved,
    fetch_active_segment_id, fetch_resume_previews, rebase_resolved_to_resumable_segment,
};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

pub fn locate_session_metadata_with_provider_dispatch(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    provider_registry: Option<&ProviderRegistry>,
    input: &str,
) -> Result<SessionMetadata, MetadataError> {
    locate_session_metadata_with_dispatch_policy(
        state,
        models,
        providers_cfg,
        sessions_cfg,
        provider_registry,
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
    let parsed_input = parse_metadata_input(input)?;
    reject_recent_ambiguity_for_policy(state, input, ambiguity_policy)?;
    let facts = load_builtin_metadata_facts(state, models, providers_cfg, sessions_cfg, input)?;
    Ok(map_builtin_session_metadata(parsed_input, facts))
}

fn locate_session_metadata_with_dispatch_policy(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    provider_registry: Option<&ProviderRegistry>,
    input: &str,
    ambiguity_policy: AmbiguityPolicy,
) -> Result<SessionMetadata, MetadataError> {
    let Some(registry) = provider_registry else {
        return locate_session_metadata_with_policy(
            state,
            models,
            providers_cfg,
            sessions_cfg,
            input,
            ambiguity_policy,
        );
    };
    let parsed_input = parse_metadata_input(input)?;
    reject_recent_ambiguity_for_policy(state, input, ambiguity_policy)?;
    let resolved = resolve_metadata_session(state, models, input)?;
    if !resolved_uses_external_provider(&resolved) {
        return locate_session_metadata_with_policy(
            state,
            models,
            providers_cfg,
            sessions_cfg,
            input,
            ambiguity_policy,
        );
    }
    locate_external_provider_session_metadata(
        state,
        providers_cfg,
        registry,
        parsed_input,
        &resolved,
    )
}

fn parse_metadata_input(input: &str) -> Result<uuid::Uuid, MetadataError> {
    parse_session_uuid(input)
}

fn reject_recent_ambiguity_for_policy(
    state: &StateDb,
    input: &str,
    ambiguity_policy: AmbiguityPolicy,
) -> Result<(), MetadataError> {
    if !rejects_recent_ambiguity(ambiguity_policy) {
        return Ok(());
    }
    let previews = metadata_db_result(fetch_resume_previews(state, input))?;
    let cutoff = recency_cutoff_for_resume_previews();
    reject_ambiguous_recent_matches(input, count_recent_previews(&previews, cutoff))
}

fn resolve_metadata_session(
    state: &StateDb,
    models: &ModelStore,
    input: &str,
) -> Result<oulipoly_state::ResolvedResume, MetadataError> {
    state
        .resolve_resume(models, input, None)
        .map_err(map_resume_error)
}

fn resolved_uses_external_provider(resolved: &oulipoly_state::ResolvedResume) -> bool {
    resolved
        .model
        .as_ref()
        .is_some_and(|model| model.provider.is_some())
}

fn locate_external_provider_session_metadata(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    registry: &ProviderRegistry,
    parsed_input: uuid::Uuid,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<SessionMetadata, MetadataError> {
    let facts = load_external_metadata_facts(state, providers_cfg, registry, resolved)?;
    Ok(map_external_session_metadata(parsed_input, resolved, facts))
}

struct BuiltinMetadataFacts {
    resolved: oulipoly_state::ResolvedResume,
    provider: oulipoly_config::ProviderConfig,
    provider_name: String,
    active_segment_id: i64,
    located_transcript: LocatedTranscript,
    workspace_root: PathBuf,
}

fn load_builtin_metadata_facts(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    input: &str,
) -> Result<BuiltinMetadataFacts, MetadataError> {
    let mut resolved = resolve_metadata_session(state, models, input)?;
    let provider =
        resolve_builtin_provider_with_rebase(state, providers_cfg, sessions_cfg, &mut resolved)?;
    let provider_name = resolved.active_provider.clone();
    let active_segment_id = external_active_segment_id(state, &resolved)?;
    let located_transcript = available_jsonl_path(
        sessions_cfg,
        provider.session_storage.as_ref(),
        &provider_name,
        &resolved.active_session_id,
    )?;
    let workspace_root = resolve_cwd_from_session_storage(
        provider.session_storage.as_ref(),
        &provider_name,
        &resolved.active_session_id,
    )?;
    Ok(builtin_metadata_facts(
        resolved,
        provider,
        provider_name,
        active_segment_id,
        located_transcript,
        workspace_root,
    ))
}

/// Resolve the effective provider for a builtin resume, rebasing the resolved
/// resume onto an earlier resumable segment when the active provider is no
/// longer configured.
fn resolve_builtin_provider_with_rebase(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    resolved: &mut ResolvedResume,
) -> Result<oulipoly_config::ProviderConfig, MetadataError> {
    if let Ok(provider) = effective_provider_for_resolved(resolved, providers_cfg) {
        return Ok(provider);
    }
    *resolved = rebase_builtin_resolved(state, providers_cfg, sessions_cfg, resolved.clone())?;
    effective_provider_for_resolved(resolved, providers_cfg)
}

fn builtin_metadata_facts(
    resolved: oulipoly_state::ResolvedResume,
    provider: oulipoly_config::ProviderConfig,
    provider_name: String,
    active_segment_id: i64,
    located_transcript: LocatedTranscript,
    workspace_root: PathBuf,
) -> BuiltinMetadataFacts {
    BuiltinMetadataFacts {
        resolved,
        provider,
        provider_name,
        active_segment_id,
        located_transcript,
        workspace_root,
    }
}

/// Rebase a resolved resume onto a resumable earlier chain segment when its
/// active provider is no longer configured (builtin storage path).
fn rebase_builtin_resolved(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    resolved: ResolvedResume,
) -> Result<ResolvedResume, MetadataError> {
    let model = resolved.model.clone();
    let model_name = resolved.model_name.clone();
    let chain_id = resolved.chain_id.clone();
    rebase_resolved_to_resumable_segment(state, resolved, |provider_name, session_id| {
        builtin_segment_resumable(
            providers_cfg,
            sessions_cfg,
            &model,
            &model_name,
            &chain_id,
            provider_name,
            session_id,
        )
    })
}

/// A builtin segment is resumable when its provider resolves and its transcript
/// is reachable under that provider's storage.
fn builtin_segment_resumable(
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    model: &Option<ModelConfig>,
    model_name: &Option<String>,
    chain_id: &str,
    provider_name: &str,
    session_id: &str,
) -> bool {
    let candidate = candidate_resolved(model, model_name, chain_id, provider_name, session_id);
    match effective_provider_for_resolved(&candidate, providers_cfg) {
        Ok(provider) => available_jsonl_path(
            sessions_cfg,
            provider.session_storage.as_ref(),
            provider_name,
            session_id,
        )
        .is_ok(),
        Err(_) => false,
    }
}

fn candidate_resolved(
    model: &Option<ModelConfig>,
    model_name: &Option<String>,
    chain_id: &str,
    provider_name: &str,
    session_id: &str,
) -> ResolvedResume {
    ResolvedResume {
        chain_id: chain_id.to_string(),
        model_name: model_name.clone(),
        model: model.clone(),
        active_provider: provider_name.to_string(),
        active_session_id: session_id.to_string(),
    }
}

fn map_builtin_session_metadata(
    parsed_input: uuid::Uuid,
    facts: BuiltinMetadataFacts,
) -> SessionMetadata {
    let storage_type = facts.located_transcript.storage_classification;
    let jsonl_path = facts.located_transcript.path;
    let active_session_uuid =
        parse_optional_uuid(&facts.resolved.active_session_id).unwrap_or(parsed_input);
    let chain_uuid = parse_optional_uuid(&facts.resolved.chain_id);
    let mutable = is_metadata_mutable(
        storage_type,
        &facts.provider,
        &jsonl_path,
        &facts.workspace_root,
    );
    session_metadata_from_parts(SessionMetadataParts {
        session_id: format_uuid(active_session_uuid),
        chain_id: format_optional_uuid(&facts.resolved.chain_id, chain_uuid),
        active_segment_id: facts.active_segment_id,
        provider_name: facts.provider_name,
        storage_type,
        jsonl_path,
        workspace_root: facts.workspace_root,
        mutable,
    })
}

struct ExternalMetadataFacts {
    provider: oulipoly_config::ProviderConfig,
    provider_name: String,
    active_segment_id: i64,
    located_transcript: LocatedTranscript,
    workspace_root: PathBuf,
}

fn load_external_metadata_facts(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    registry: &ProviderRegistry,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<ExternalMetadataFacts, MetadataError> {
    let provider = effective_provider_for_resolved(resolved, providers_cfg)?;
    let provider_name = resolved.active_provider.clone();
    let model_name = external_resolved_model_name(resolved)?;
    let active_segment_id = external_active_segment_id(state, resolved)?;
    let identity = external_locate_identity(registry, model_name, &provider_name)?;
    let located_transcript =
        locate_external_provider_transcript(registry, identity, &resolved.active_session_id)?;
    let workspace_root = external_workspace_root(
        provider.session_storage.as_ref(),
        &provider_name,
        &resolved.active_session_id,
    );
    Ok(external_metadata_facts(
        provider,
        provider_name,
        active_segment_id,
        located_transcript,
        workspace_root,
    ))
}

fn external_metadata_facts(
    provider: oulipoly_config::ProviderConfig,
    provider_name: String,
    active_segment_id: i64,
    located_transcript: LocatedTranscript,
    workspace_root: PathBuf,
) -> ExternalMetadataFacts {
    ExternalMetadataFacts {
        provider,
        provider_name,
        active_segment_id,
        located_transcript,
        workspace_root,
    }
}

fn map_external_session_metadata(
    parsed_input: uuid::Uuid,
    resolved: &oulipoly_state::ResolvedResume,
    facts: ExternalMetadataFacts,
) -> SessionMetadata {
    let storage_type = facts.located_transcript.storage_classification;
    let jsonl_path = facts.located_transcript.path;
    let active_session_uuid =
        parse_optional_uuid(&resolved.active_session_id).unwrap_or(parsed_input);
    let chain_uuid = parse_optional_uuid(&resolved.chain_id);
    let mutable = is_metadata_mutable(
        storage_type,
        &facts.provider,
        &jsonl_path,
        &facts.workspace_root,
    );
    session_metadata_from_parts(SessionMetadataParts {
        session_id: format_uuid(active_session_uuid),
        chain_id: format_optional_uuid(&resolved.chain_id, chain_uuid),
        active_segment_id: facts.active_segment_id,
        provider_name: facts.provider_name,
        storage_type,
        jsonl_path,
        workspace_root: facts.workspace_root,
        mutable,
    })
}

fn external_resolved_model_name(
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<&str, MetadataError> {
    resolved
        .model_name
        .as_deref()
        .ok_or_else(|| MetadataError::Operational {
            message: "session_provider_model_name_missing".to_string(),
        })
}

fn external_active_segment_id(
    state: &StateDb,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<i64, MetadataError> {
    active_segment_id_to_metadata_error_or_value(
        metadata_db_result(fetch_active_segment_id(state, resolved))?,
        &resolved.chain_id,
    )
}

fn locate_external_provider_transcript(
    registry: &ProviderRegistry,
    identity: SessionProviderIdentity,
    session_id: &str,
) -> Result<LocatedTranscript, MetadataError> {
    session_provider::locate_transcript(SessionProviderLocateRequest {
        registry,
        identity,
        session_id,
        lookup_mode: TranscriptLookupMode::RequireExisting,
        effective_cwd: None,
        purpose: None,
        tail_bytes_hint: None,
    })
    .map_err(session_provider_metadata_error)
}

fn external_locate_identity(
    registry: &ProviderRegistry,
    model_name: &str,
    provider_name: &str,
) -> Result<SessionProviderIdentity, MetadataError> {
    let describe = external_locate_describe(registry, model_name)?;
    Ok(external_identity_from_describe(
        model_name,
        provider_name,
        describe,
    ))
}

fn external_locate_describe(
    registry: &ProviderRegistry,
    model_name: &str,
) -> Result<oulipoly_provider::generated::DescribeResult, MetadataError> {
    registry
        .describe_model_provider(model_name)
        .map_err(describe_model_provider_error)
}

fn describe_model_provider_error<E: std::fmt::Display>(error: E) -> MetadataError {
    MetadataError::Operational {
        message: describe_model_provider_failed_message(error),
    }
}

fn describe_model_provider_failed_message<E: std::fmt::Display>(error: E) -> String {
    format!("session_provider_describe_failed: {error}")
}

fn external_identity_from_describe(
    model_name: &str,
    provider_name: &str,
    describe: oulipoly_provider::generated::DescribeResult,
) -> SessionProviderIdentity {
    SessionProviderIdentity {
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_instance_id: Some(format_external_provider_instance_id(&describe.provider_id)),
        settings_id: external_settings_id(describe.settings_schema_id),
    }
}

fn format_external_provider_instance_id(provider_id: &str) -> String {
    format!("{provider_id}-instance")
}

fn external_settings_id(settings_schema_id: Option<String>) -> String {
    settings_schema_id.unwrap_or_else(|| session_provider::S7A_NEUTRAL_SETTINGS_ID.to_string())
}

fn external_workspace_root(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> PathBuf {
    resolve_cwd_from_session_storage(session_storage, provider_name, session_id)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn session_provider_metadata_error(error: session_provider::SessionProviderError) -> MetadataError {
    MetadataError::Operational {
        message: error.to_string(),
    }
}

pub fn resolve_resume_workspace_root(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    input: &str,
) -> Result<PathBuf, MetadataError> {
    let resolved = state
        .resolve_resume(models, input, None)
        .map_err(map_resume_error)?;
    let provider = effective_provider_for_resolved(&resolved, providers_cfg)?;
    if let Some(workspace_root) =
        invocation_resolved_workspace_root(state, provider.session_storage.as_ref(), &resolved)?
    {
        return Ok(workspace_root);
    }
    if let Some(workspace_root) = session_runtime_workspace_root(&resolved)? {
        return Ok(workspace_root);
    }
    resolve_cwd_from_session_storage(
        provider.session_storage.as_ref(),
        &resolved.active_provider,
        &resolved.active_session_id,
    )
}

fn session_runtime_workspace_root(
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<Option<PathBuf>, MetadataError> {
    let Some(db) = external_session_runtime_db()? else {
        return Ok(None);
    };
    let Some(row) = db
        .session_runtime(&resolved.active_session_id)
        .map_err(external_session_runtime_error)?
    else {
        return Ok(None);
    };
    row.effective_cwd
        .map(|value| validate_stored_workspace_root(value, "session_runtime_effective_cwd"))
        .transpose()
}

fn invocation_resolved_workspace_root(
    state: &StateDb,
    session_storage: Option<&SessionStorage>,
    resolved: &oulipoly_state::ResolvedResume,
) -> Result<Option<PathBuf>, MetadataError> {
    if !supports_invocation_workspace_fallback(session_storage) {
        return Ok(None);
    }
    let value = metadata_db_result(state.latest_provider_session_resolved_account(
        &resolved.active_provider,
        &resolved.active_session_id,
    ))?;
    value
        .map(|value| validate_stored_workspace_root(value, "provider_session_resolved_account"))
        .transpose()
}

fn supports_invocation_workspace_fallback(session_storage: Option<&SessionStorage>) -> bool {
    matches!(session_storage, Some(SessionStorage::Script { .. }))
}

fn external_session_runtime_db() -> Result<Option<oulipoly_state::mailbox::MailboxDb>, MetadataError>
{
    oulipoly_state::mailbox::MailboxDb::open_default_if_exists()
        .map_err(external_session_runtime_error)
}

fn validate_stored_workspace_root(value: String, source: &str) -> Result<PathBuf, MetadataError> {
    require_absolute_stored_workspace_root(stored_workspace_root_path(value), source)
}

fn stored_workspace_root_path(value: String) -> PathBuf {
    PathBuf::from(value)
}

fn require_absolute_stored_workspace_root(
    path: PathBuf,
    source: &str,
) -> Result<PathBuf, MetadataError> {
    if stored_workspace_root_is_absolute(&path) {
        Ok(path)
    } else {
        Err(stored_workspace_root_not_absolute(&path, source))
    }
}

fn stored_workspace_root_is_absolute(path: &std::path::Path) -> bool {
    path.is_absolute()
}

fn stored_workspace_root_not_absolute(path: &std::path::Path, source: &str) -> MetadataError {
    MetadataError::Operational {
        message: stored_workspace_root_not_absolute_message(path, source),
    }
}

fn stored_workspace_root_not_absolute_message(path: &std::path::Path, source: &str) -> String {
    format!("{source}_not_absolute: {}", path.display())
}

fn external_session_runtime_error(message: String) -> MetadataError {
    MetadataError::Operational { message }
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
