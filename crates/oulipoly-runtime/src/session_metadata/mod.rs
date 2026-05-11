use crate::sessions::locate_transcript;
use oulipoly_config::{
    ProviderConfig, ProvidersConfig, ScriptSessionStorageType, SessionStorage, SessionsConfig,
};
use oulipoly_state::{ModelStore, ResumeError, StateDb};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    let parsed_input = Uuid::parse_str(input).map_err(|_| MetadataError::InvalidSessionId {
        input: input.to_string(),
    })?;

    if matches!(ambiguity_policy, AmbiguityPolicy::Reject) {
        let previews = state
            .resume_previews(input)
            .map_err(|message| MetadataError::Operational { message })?;
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
        let recent_count = previews
            .iter()
            .filter(|preview| preview.last_used_at >= cutoff)
            .count();
        if recent_count > 1 {
            return Err(MetadataError::AmbiguousSession {
                input: input.to_string(),
            });
        }
    }

    let resolved = state
        .resolve_resume(models, input, None)
        .map_err(map_resume_error)?;
    let provider = effective_provider_for_resolved(&resolved, providers_cfg)?;
    let provider_name = resolved.active_provider.clone();
    let storage_type = SessionStorageType::from(&provider.session_storage);
    let active_segment_id = state
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
        sessions_cfg,
        provider.session_storage.as_ref(),
        &provider_name,
        &resolved.active_session_id,
        true,
    )?;
    let workspace_root = resolve_cwd_from_session_storage(
        provider.session_storage.as_ref(),
        &provider_name,
        &resolved.active_session_id,
    )?;

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

pub fn resolve_resume_workspace_root(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    input: &str,
) -> Result<PathBuf, MetadataError> {
    Uuid::parse_str(input).map_err(|_| MetadataError::InvalidSessionId {
        input: input.to_string(),
    })?;
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
    resolved: &oulipoly_state::ResolvedResume,
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
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
    require_existing_file: bool,
) -> Result<PathBuf, MetadataError> {
    match locate_transcript(sessions_cfg, provider_name, session_id) {
        Ok(Some(path)) => validate_jsonl_path(provider_name, path, require_existing_file),
        Ok(None) => locate_jsonl_path_from_storage(
            session_storage,
            provider_name,
            session_id,
            require_existing_file,
        ),
        Err(message) => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("locator_error: {message}"),
        }),
    }
}

pub(crate) fn resolve_jsonl_path_for_provider_allow_missing(
    sessions_cfg: &SessionsConfig,
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    available_jsonl_path(
        sessions_cfg,
        session_storage,
        provider_name,
        session_id,
        false,
    )
}

fn validate_jsonl_path(
    provider_name: &str,
    path: PathBuf,
    require_existing_file: bool,
) -> Result<PathBuf, MetadataError> {
    if !path.is_absolute() {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("relative_jsonl_path: {}", path.display()),
        });
    }
    if !path.exists() {
        if !require_existing_file {
            if path.to_str().is_none() {
                return Err(MetadataError::UnsupportedStorage {
                    provider_name: provider_name.to_string(),
                    reason: "non_utf8_jsonl_path".to_string(),
                });
            }
            return Ok(path);
        }
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

fn locate_jsonl_path_from_storage(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
    require_existing_file: bool,
) -> Result<PathBuf, MetadataError> {
    match session_storage {
        Some(SessionStorage::Script {
            transcript_script: Some(script),
            ..
        }) => locate_jsonl_path_from_transcript_script(
            script,
            provider_name,
            session_id,
            require_existing_file,
        ),
        Some(SessionStorage::Script { .. }) => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "no_transcript_script_for_script_storage".to_string(),
        }),
        Some(SessionStorage::ClaudeCode { projects_dir }) => {
            locate_claude_jsonl_path_from_storage(projects_dir, provider_name, session_id)
        }
        Some(SessionStorage::Codex { sessions_dir }) => {
            locate_codex_jsonl_path_from_storage(sessions_dir, provider_name, session_id)
        }
        None => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "no_locator".to_string(),
        }),
    }
}

fn locate_jsonl_path_from_transcript_script(
    script: &str,
    provider_name: &str,
    session_id: &str,
    require_existing_file: bool,
) -> Result<PathBuf, MetadataError> {
    let stdout =
        run_session_id_script(script, session_id, "transcript_script").map_err(|reason| {
            MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason,
            }
        })?;
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    match lines.as_slice() {
        [] => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "transcript_script_empty_stdout".to_string(),
        }),
        [line] => validate_jsonl_path(provider_name, PathBuf::from(line), require_existing_file),
        _ => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "transcript_script_stdout_not_single_line".to_string(),
        }),
    }
}

fn locate_claude_jsonl_path_from_storage(
    projects_dir: &Path,
    provider_name: &str,
    session_id: &str,
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
    let filename = format!("{session_id}.jsonl");
    let mut matches = Vec::new();
    let entries = fs::read_dir(&projects_dir).map_err(|e| MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason: format!(
            "claude_projects_dir_unavailable: {}: {e}",
            projects_dir.display()
        ),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!(
                "claude_projects_dir_read_error: {}: {e}",
                projects_dir.display()
            ),
        })?;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            matches.push(validate_jsonl_path(provider_name, candidate, true)?);
        }
    }
    single_jsonl_match(provider_name, "claude_storage_scan", matches)
}

fn locate_codex_jsonl_path_from_storage(
    sessions_dir: &Path,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    let sessions_dir =
        sessions_dir
            .canonicalize()
            .map_err(|e| MetadataError::UnsupportedStorage {
                provider_name: provider_name.to_string(),
                reason: format!(
                    "codex_sessions_dir_unavailable: {}: {e}",
                    sessions_dir.display()
                ),
            })?;
    let mut matches = Vec::new();
    collect_codex_rollout_matches(&sessions_dir, provider_name, session_id, 0, &mut matches)?;
    single_jsonl_match(provider_name, "codex_storage_scan", matches)
}

fn collect_codex_rollout_matches(
    dir: &Path,
    provider_name: &str,
    session_id: &str,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), MetadataError> {
    if depth > 4 {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| MetadataError::UnsupportedStorage {
        provider_name: provider_name.to_string(),
        reason: format!("codex_sessions_dir_read_error: {}: {e}", dir.display()),
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("codex_sessions_dir_read_error: {}: {e}", dir.display()),
        })?;
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_codex_rollout_matches(&path, provider_name, session_id, depth + 1, matches)?;
        } else if file_type.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with("rollout-")
                        && name.ends_with(".jsonl")
                        && name.contains(session_id)
                })
        {
            matches.push(validate_jsonl_path(provider_name, path, true)?);
        }
    }
    Ok(())
}

fn single_jsonl_match(
    provider_name: &str,
    source: &str,
    mut matches: Vec<PathBuf>,
) -> Result<PathBuf, MetadataError> {
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("{source}_not_found"),
        }),
        _ => Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("{source}_ambiguous"),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct CwdScriptResponse {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    error: Option<String>,
}

fn resolve_cwd_from_session_storage(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    let Some(session_storage) = session_storage else {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "no_workspace_root_for_other_storage: storage_type other".to_string(),
        });
    };

    let script = session_storage.cwd_script();
    let stdout = run_cwd_script(&script, session_id).map_err(|reason| {
        MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason,
        }
    })?;
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let [line] = lines.as_slice() else {
        let reason = if lines.is_empty() {
            "cwd_script_empty_stdout"
        } else {
            "cwd_script_stdout_not_single_line"
        };
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: reason.to_string(),
        });
    };
    let response: CwdScriptResponse =
        serde_json::from_str(line).map_err(|e| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("cwd_script_malformed_json: {e}"),
        })?;
    if let Some(error) = response.error.as_deref()
        && !error.trim().is_empty()
    {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("cwd_script_error: {error}"),
        });
    }
    if !response.found {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "cwd_script_not_found".to_string(),
        });
    }
    let cwd = response
        .cwd
        .as_deref()
        .ok_or_else(|| MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: "cwd_script_missing_cwd".to_string(),
        })?;
    let cwd = PathBuf::from(cwd);
    if !cwd.is_absolute() {
        return Err(MetadataError::UnsupportedStorage {
            provider_name: provider_name.to_string(),
            reason: format!("cwd_script_cwd_not_absolute: {}", cwd.display()),
        });
    }
    Ok(cwd)
}

pub(crate) fn resolve_workspace_root_for_provider_session(
    session_storage: Option<&SessionStorage>,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, MetadataError> {
    resolve_cwd_from_session_storage(session_storage, provider_name, session_id)
}

fn run_cwd_script(script: &str, session_id: &str) -> Result<String, String> {
    run_session_id_script(script, session_id, "cwd_script")
}

fn run_session_id_script(
    script: &str,
    session_id: &str,
    script_kind: &str,
) -> Result<String, String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("{script} \"$1\""))
        .arg("oulipoly-session-script")
        .arg(session_id)
        .env("SESSION_ID", session_id)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{script_kind}_spawn_failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{script_kind}_exit_{}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn normalize_uuid(input: &str) -> Option<String> {
    Uuid::parse_str(input).ok().map(|uuid| uuid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oulipoly_config::{ProviderEntry, ResumeKind, ResumeStrategy};
    use std::os::unix::fs::PermissionsExt;

    struct FixtureScript {
        _dir: tempfile::TempDir,
        path: PathBuf,
    }

    fn fixture_script(body: &str) -> FixtureScript {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cwd-script.sh");
        std::fs::write(&path, format!("#!/usr/bin/env bash\n{body}\n")).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        FixtureScript { _dir: dir, path }
    }

    fn state_with_session(provider_name: &str, session_id: &str) -> StateDb {
        let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        db.mint_imported_chain_if_absent(provider_name, session_id, &Utc::now(), "<unknown>")
            .unwrap();
        db
    }

    fn providers_cfg(provider_name: &str, cwd_script: String) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        cfg.entries.insert(
            provider_name.to_string(),
            ProviderEntry {
                command: Some("provider-fixture".to_string()),
                resume: Some(ResumeStrategy {
                    kind: ResumeKind::Flag,
                    flag: Some("--resume".to_string()),
                    subcommand: None,
                }),
                session_storage: Some(SessionStorage::Script {
                    cwd_script,
                    transcript_script: None,
                    storage_type: None,
                }),
                ..ProviderEntry::default()
            },
        );
        cfg
    }

    fn providers_cfg_with_storage(
        provider_name: &str,
        cwd_script: String,
        transcript_script: String,
        storage_type: ScriptSessionStorageType,
    ) -> ProvidersConfig {
        let mut cfg = ProvidersConfig::default();
        cfg.entries.insert(
            provider_name.to_string(),
            ProviderEntry {
                command: Some("provider-fixture".to_string()),
                resume: Some(ResumeStrategy {
                    kind: ResumeKind::Flag,
                    flag: Some("--resume".to_string()),
                    subcommand: None,
                }),
                session_storage: Some(SessionStorage::Script {
                    cwd_script,
                    transcript_script: Some(transcript_script),
                    storage_type: Some(storage_type),
                }),
                ..ProviderEntry::default()
            },
        );
        cfg
    }

    #[test]
    fn resolve_resume_workspace_root_uses_cwd_script_response() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let script = fixture_script(&format!(
            "printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n'",
            workspace.display()
        ));
        let provider_name = "provider";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let db = state_with_session(provider_name, session_id);
        let cfg = providers_cfg(provider_name, script.path.display().to_string());

        let resolved =
            resolve_resume_workspace_root(&db, &ModelStore::new(), &cfg, session_id).unwrap();

        assert_eq!(resolved, workspace);
    }

    #[test]
    fn resolve_resume_workspace_root_reports_cwd_script_not_found() {
        let script = fixture_script("printf '{\"found\":false}\\n'");
        let provider_name = "provider";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let db = state_with_session(provider_name, session_id);
        let cfg = providers_cfg(provider_name, script.path.display().to_string());

        let err =
            resolve_resume_workspace_root(&db, &ModelStore::new(), &cfg, session_id).unwrap_err();

        assert!(metadata_error_reason(&err).contains("cwd_script_not_found"));
    }

    #[test]
    fn resolve_resume_workspace_root_reports_malformed_cwd_script_json() {
        let script = fixture_script("printf 'not-json\\n'");
        let provider_name = "provider";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let db = state_with_session(provider_name, session_id);
        let cfg = providers_cfg(provider_name, script.path.display().to_string());

        let err =
            resolve_resume_workspace_root(&db, &ModelStore::new(), &cfg, session_id).unwrap_err();

        assert!(metadata_error_reason(&err).contains("cwd_script_malformed_json"));
    }

    #[test]
    fn locate_session_metadata_uses_script_storage_transcript_and_format() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let jsonl_path = dir.path().join("session.jsonl");
        std::fs::write(&jsonl_path, "{}\n").unwrap();
        let provider_name = "provider";
        let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
        let cwd_script = fixture_script(&format!(
            "printf '{{\"found\":true,\"cwd\":\"{}\"}}\\n'",
            workspace.display()
        ));
        let transcript_script = fixture_script(&format!(
            "test \"$SESSION_ID\" = '{}' || exit 7\nprintf '{}\\n'",
            session_id,
            jsonl_path.display()
        ));
        let db = state_with_session(provider_name, session_id);
        let cfg = providers_cfg_with_storage(
            provider_name,
            cwd_script.path.display().to_string(),
            transcript_script.path.display().to_string(),
            ScriptSessionStorageType::ClaudeCode,
        );

        let metadata = locate_session_metadata(
            &db,
            &ModelStore::new(),
            &cfg,
            &SessionsConfig::default(),
            session_id,
        )
        .unwrap();

        assert_eq!(metadata.storage_type, SessionStorageType::ClaudeCode);
        assert_eq!(metadata.jsonl_path, jsonl_path.canonicalize().unwrap());
        assert_eq!(metadata.workspace_root, workspace);
        assert!(metadata.mutable);
    }

    fn metadata_error_reason(err: &MetadataError) -> &str {
        match err {
            MetadataError::UnsupportedStorage { reason, .. } => reason,
            other => panic!("expected unsupported storage, got {other:?}"),
        }
    }
}
