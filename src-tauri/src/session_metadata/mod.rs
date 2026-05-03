use crate::config::{
    ModelConfigRepository, ProviderConfig, ProviderConfigSource, ProvidersConfig, SessionStorage,
    SessionsConfig, SessionsConfigSource,
};
use crate::process::ProcessRunner;
use crate::sessions::locate_transcript_with_runner;
use crate::state::{ResolvedResume, ResumeError, SessionChainRepository};
use serde::Serialize;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
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
    chain_repo: &dyn SessionChainRepository,
    model_repo: &dyn ModelConfigRepository,
    provider_source: &dyn ProviderConfigSource,
    sessions_source: &dyn SessionsConfigSource,
    locator_runner: &dyn ProcessRunner,
    input: &str,
) -> Result<SessionMetadata, MetadataError> {
    let parsed_input = Uuid::parse_str(input).map_err(|_| MetadataError::InvalidSessionId {
        input: input.to_string(),
    })?;

    let facts = chain_repo
        .resolve_resume_facts(input, None)
        .map_err(map_resume_error)?;
    let models = model_repo
        .load_models()
        .map_err(|message| MetadataError::Operational { message })?;
    let providers_cfg = provider_source
        .load_providers()
        .map_err(|message| MetadataError::Operational { message })?;
    let sessions_cfg = sessions_source
        .load_sessions()
        .map_err(|message| MetadataError::Operational { message })?;
    let resolved = resolved_from_facts(facts, &models)?;
    let provider = effective_provider_for_resolved(&resolved, &providers_cfg)?;
    let provider_name = resolved.active_provider.clone();
    let storage_type = SessionStorageType::from(&provider.session_storage);
    let active_segment_id = chain_repo
        .active_segment_id_for_chain_provider_session(
            &resolved.chain_id,
            &resolved.active_provider,
            &resolved.active_session_id,
        )
        .map_err(|message| MetadataError::Operational { message })?
        .ok_or_else(|| MetadataError::SessionNotFound {
            input: resolved.chain_id.clone(),
        })?;

    let jsonl_path = available_jsonl_path(
        &sessions_cfg,
        &provider_name,
        &resolved.active_session_id,
        locator_runner,
    )?;
    let workspace_root = match &provider.session_storage {
        Some(SessionStorage::ClaudeCode { projects_dir }) => {
            derive_claude_workspace_root(projects_dir, &jsonl_path, &provider_name)?
        }
        Some(SessionStorage::Codex { .. }) => {
            derive_codex_workspace_root(&jsonl_path, &provider_name)?
        }
        None => {
            return Err(MetadataError::UnsupportedStorage {
                provider_name,
                reason: "no_workspace_root_for_other_storage: storage_type other".to_string(),
            });
        }
    };

    let mutable = storage_type != SessionStorageType::Other
        && provider.resume.is_some()
        && jsonl_path.is_absolute()
        && workspace_root.is_absolute();

    Ok(SessionMetadata {
        session_id: normalize_uuid(&resolved.active_session_id)
            .unwrap_or_else(|| parsed_input.to_string()),
        chain_id: normalize_uuid(&resolved.chain_id).unwrap_or_else(|| resolved.chain_id.clone()),
        active_segment_id,
        provider_name,
        storage_type,
        jsonl_path,
        workspace_root,
        transcript_state: TranscriptState::Available,
        mutable,
    })
}

fn resolved_from_facts(
    facts: crate::state::ResumeDbFacts,
    models: &std::collections::HashMap<String, crate::config::ModelConfig>,
) -> Result<ResolvedResume, MetadataError> {
    let model = match &facts.inferred_model_name {
        Some(model_name) => {
            Some(
                models
                    .get(model_name)
                    .cloned()
                    .ok_or_else(|| MetadataError::Operational {
                        message: format!("unknown model {model_name}"),
                    })?,
            )
        }
        None => None,
    };
    if let Some(model) = &model
        && !model
            .providers
            .iter()
            .any(|provider| provider.name == facts.active_provider)
    {
        return Err(MetadataError::Operational {
            message: format!(
                "model {} does not include active provider {}",
                model.name, facts.active_provider
            ),
        });
    }
    Ok(ResolvedResume {
        chain_id: facts.chain_id,
        model_name: facts.inferred_model_name,
        model,
        active_provider: facts.active_provider,
        active_session_id: facts.active_session_id,
    })
}

fn map_resume_error(err: ResumeError) -> MetadataError {
    match err {
        ResumeError::InvalidUuid { input } => MetadataError::InvalidSessionId { input },
        ResumeError::NoChainFound { input } => MetadataError::SessionNotFound { input },
        ResumeError::Ambiguous { input, .. } => MetadataError::AmbiguousSession { input },
        ResumeError::UnknownModel { model_name } => MetadataError::Operational {
            message: format!("unknown model {model_name}"),
        },
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            ..
        } => MetadataError::Operational {
            message: format!(
                "model {model_name} does not include active provider {active_provider}"
            ),
        },
        ResumeError::ProviderNotConfigured { provider } => MetadataError::UnsupportedStorage {
            provider_name: provider.clone(),
            reason: format!("provider {provider} is not configured"),
        },
        ResumeError::ActiveSegmentMissing { chain_id } => {
            MetadataError::SessionNotFound { input: chain_id }
        }
        ResumeError::ProviderMissingResume { provider_name } => MetadataError::UnsupportedStorage {
            provider_name: provider_name.clone(),
            reason: format!("provider {provider_name} has no resume strategy"),
        },
        ResumeError::Db { message } => MetadataError::Operational { message },
    }
}

fn effective_provider_for_resolved(
    resolved: &crate::state::ResolvedResume,
    providers_cfg: &ProvidersConfig,
) -> Result<ProviderConfig, MetadataError> {
    if let Some(model) = resolved.model.as_ref() {
        let model_provider = model
            .providers
            .iter()
            .find(|provider| provider.name == resolved.active_provider)
            .ok_or_else(|| MetadataError::Operational {
                message: format!(
                    "model {} does not include active provider {}",
                    model.name, resolved.active_provider
                ),
            })?;
        let (provider, _) =
            providers_cfg
                .effective_provider(model_provider)
                .map_err(|message| MetadataError::UnsupportedStorage {
                    provider_name: resolved.active_provider.clone(),
                    reason: message,
                })?;
        Ok(provider)
    } else {
        let (provider, _) = providers_cfg
            .runtime_provider(&resolved.active_provider)
            .map_err(|message| MetadataError::UnsupportedStorage {
                provider_name: resolved.active_provider.clone(),
                reason: message,
            })?;
        Ok(provider)
    }
}

fn available_jsonl_path(
    sessions_cfg: &SessionsConfig,
    provider_name: &str,
    session_id: &str,
    locator_runner: &dyn ProcessRunner,
) -> Result<PathBuf, MetadataError> {
    let path =
        locate_transcript_with_runner(sessions_cfg, provider_name, session_id, locator_runner)
            .map_err(|message| MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!("locator_error: {message}"),
            })?;
    let Some(path) = path else {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "no_locator".to_string(),
        });
    };
    if !path.is_absolute() {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("relative_jsonl_path: {}", path.display()),
        });
    }
    if !path.exists() {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("missing_jsonl_path: {}", path.display()),
        });
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("missing_jsonl_path: {}: {e}", path.display()),
        })?;
    if canonical.to_str().is_none() {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "non_utf8_jsonl_path".to_string(),
        });
    }
    Ok(canonical)
}

fn derive_claude_workspace_root(
    projects_dir: &Path,
    jsonl_path: &Path,
    provider_name: &str,
) -> Result<PathBuf, MetadataError> {
    let projects_dir =
        projects_dir
            .canonicalize()
            .map_err(|e| MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!(
                    "claude_projects_dir_unavailable: {}: {e}",
                    projects_dir.display()
                ),
            })?;
    let transcript_dir = jsonl_path
        .parent()
        .ok_or_else(|| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("claude_jsonl_path_malformed: {}", jsonl_path.display()),
        })?;
    let transcript_parent =
        transcript_dir
            .parent()
            .ok_or_else(|| MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!("claude_jsonl_path_malformed: {}", jsonl_path.display()),
            })?;
    if transcript_parent != projects_dir {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!(
                "claude_jsonl_outside_projects_dir: {}",
                jsonl_path.display()
            ),
        });
    }
    let encoded = transcript_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "claude_non_utf8_project_dir".to_string(),
        })?;
    let candidates = decode_claude_project_dir_candidates(encoded);
    let mut existing = Vec::new();
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        let canonical =
            candidate
                .canonicalize()
                .map_err(|e| MetadataError::UnsupportedStorage {
                    provider_name: provider_name.to_string(),
                    reason: format!(
                        "claude_workspace_canonicalize_failed: {}: {e}",
                        candidate.display()
                    ),
                })?;
        if canonical.to_str().is_none() {
            return Err(MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: "claude_non_utf8_workspace_root".to_string(),
            });
        }
        existing.push(canonical);
    }
    match existing.len() {
        1 => Ok(existing.remove(0)),
        0 => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "no_existing_path_hash_decomposition".to_string(),
        }),
        _ => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "ambiguous_path_hash".to_string(),
        }),
    }
}

fn decode_claude_project_dir_candidates(encoded: &str) -> Vec<PathBuf> {
    let Some(rest) = encoded.strip_prefix('-') else {
        return Vec::new();
    };
    if rest.is_empty() {
        return vec![PathBuf::from("/")];
    }

    let mut candidates = Vec::new();
    let mut components = Vec::new();
    let mut current = String::new();
    decode_claude_rest(rest, &mut components, &mut current, &mut candidates);
    candidates
}

fn decode_claude_rest(
    remaining: &str,
    components: &mut Vec<String>,
    current: &mut String,
    candidates: &mut Vec<PathBuf>,
) {
    let Some(ch) = remaining.chars().next() else {
        if current.is_empty() {
            return;
        }
        components.push(current.clone());
        let mut path = PathBuf::from("/");
        for component in components.iter() {
            path.push(component);
        }
        candidates.push(path);
        components.pop();
        return;
    };
    let next = &remaining[ch.len_utf8()..];
    if ch == '-' {
        if !current.is_empty() {
            components.push(current.clone());
            current.clear();
            decode_claude_rest(next, components, current, candidates);
            *current = components.pop().unwrap();
        }
        current.push('-');
        decode_claude_rest(next, components, current, candidates);
        current.pop();
    } else {
        current.push(ch);
        decode_claude_rest(next, components, current, candidates);
        current.pop();
    }
}

fn derive_codex_workspace_root(
    jsonl_path: &Path,
    provider_name: &str,
) -> Result<PathBuf, MetadataError> {
    let file = File::open(jsonl_path).map_err(|e| MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason: format!("codex_read_error: {}: {e}", jsonl_path.display()),
    })?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let line = line.map_err(|e| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("codex_read_error: {}: {e}", jsonl_path.display()),
        })?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(line).map_err(|e| MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!("codex_malformed_json: {e}"),
            })?;
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let cwd = value
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(Value::as_str)
            .ok_or_else(|| MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: "codex_missing_cwd".to_string(),
            })?;
        let path = PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err(MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!("codex_cwd_not_absolute: {cwd}"),
            });
        }
        if !path.exists() {
            return Err(MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!("codex_cwd_missing: {cwd}"),
            });
        }
        let canonical = path
            .canonicalize()
            .map_err(|e| MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!("codex_cwd_missing: {cwd}: {e}"),
            })?;
        if canonical.to_str().is_none() {
            return Err(MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: "codex_cwd_non_utf8".to_string(),
            });
        }
        return Ok(canonical);
    }

    Err(MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason: "codex_missing_session_meta".to_string(),
    })
}

fn normalize_uuid(input: &str) -> Option<String> {
    Uuid::parse_str(input).ok().map(|uuid| uuid.to_string())
}
