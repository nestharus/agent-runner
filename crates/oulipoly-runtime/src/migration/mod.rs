//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`, `predicate`, `formatter`

use crate::balancer::TransitionReason;
use crate::sessions::locate_transcript;
use oulipoly_config::{ModelConfig, ScriptSessionStorageType, SessionStorage, SessionsConfig};
use oulipoly_state::{ChainSegmentRotationInput, ResolvedResume, StateDb};
use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_FRESH_SESSION_ID_ATTEMPTS: usize = 16;

#[derive(Debug, Clone)]
pub enum MigrationError {
    CodexMigrationDeferred {
        provider: String,
    },
    SourceMissing {
        provider: String,
        session_id: String,
    },
    SourcePathMalformed {
        provider: String,
        path: String,
    },
    SourceMissingStorage {
        provider: String,
    },
    TargetMissingStorage {
        provider: String,
    },
    SpawnCwdUnsupported {
        provider: String,
        cwd: String,
    },
    TargetAlreadyExists {
        provider: String,
        path: String,
    },
    TargetSessionInUseByOtherChain {
        provider: String,
        session_id: String,
        conflicting_chain_id: String,
    },
    TargetDirectoryCreateFailed {
        path: String,
        message: String,
    },
    TranscriptLocatorFailed {
        provider: String,
        message: String,
    },
    CompactionBoundaryNotInJsonl {
        session_id: String,
        turn_id: String,
    },
    ConcurrentSegmentClosed {
        chain_id: String,
    },
    ProviderNotInModelPool {
        provider: String,
        model_name: String,
    },
    ProviderMissingResume {
        provider: String,
    },
    Io {
        path: String,
        message: String,
    },
    Db {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigratedSegment {
    pub chain_id: String,
    pub source_provider: String,
    pub source_session_id: String,
    pub target_provider: String,
    pub target_provider_index: usize,
    pub target_session_id: String,
    pub target_jsonl_path: PathBuf,
    pub reason: TransitionReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRefBoundOutcome {
    NoBoundary,
    BoundaryNotFound,
    AlreadyBounded,
    Rotated(MigratedSegment),
}

enum BoundaryTailSelection<'a> {
    NoBoundary,
    BoundaryNotFound {
        turn_id: String,
    },
    Found {
        located_source_offset: Option<usize>,
        slice: Cow<'a, [u8]>,
    },
}

#[allow(clippy::too_many_arguments)]
pub fn migrate_chain_segment(
    state: &StateDb,
    sessions_cfg: &SessionsConfig,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    resume_working_dir: &Path,
    target_provider_index: usize,
    reason: TransitionReason,
    stderr: &mut dyn Write,
) -> Result<MigratedSegment, MigrationError> {
    let source = source_provider_for_model(model, resolved)?;
    let target = target_provider_for_index(model, target_provider_index)?;
    require_provider_resume(source)?;
    require_provider_resume(target)?;
    ensure_migration_storage_supported(source, target)?;
    let source_path = locate_migration_source_path(
        sessions_cfg,
        source,
        &resolved.active_session_id,
        Some(resume_working_dir),
        stderr,
    )?;
    let target_session_id = resolved.active_session_id.clone();
    let target_path = target_jsonl_path(target, resume_working_dir, &target_session_id, stderr)?;
    let bytes = read_jsonl_bytes(&source_path)?;
    let slice = explicit_migration_slice(
        state,
        source,
        &resolved.active_session_id,
        &source_path,
        &bytes,
        stderr,
    )?;
    ensure_no_conflicting_active_segment(
        state,
        &target.name,
        &target_session_id,
        &resolved.chain_id,
    )?;
    write_jsonl_atomic(&target_path, slice.as_ref())?;

    let now = chrono::Utc::now();
    state
        .close_active_segment_returning(&resolved.chain_id, &now)
        .map_err(|message| MigrationError::Db { message })?
        .ok_or_else(|| MigrationError::ConcurrentSegmentClosed {
            chain_id: resolved.chain_id.clone(),
        })?;
    state
        .open_chain_segment(
            &resolved.chain_id,
            &target.name,
            &target_session_id,
            &now,
            reason,
        )
        .map_err(|message| MigrationError::Db { message })?;
    writeln!(
        stderr,
        "[migrate] {} -> {} reason={}",
        source.name,
        target.name,
        reason.as_str()
    )
    .map_err(|e| MigrationError::Io {
        path: "<stderr>".to_string(),
        message: e.to_string(),
    })?;

    Ok(MigratedSegment {
        chain_id: resolved.chain_id.clone(),
        source_provider: source.name.clone(),
        source_session_id: resolved.active_session_id.clone(),
        target_provider: target.name.clone(),
        target_provider_index,
        target_session_id,
        target_jsonl_path: target_path,
        reason,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn bound_provider_ref_resume_segment(
    state: &StateDb,
    sessions_cfg: &SessionsConfig,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    resume_working_dir: &Path,
    fresh_session_id: &mut dyn FnMut() -> String,
    stderr: &mut dyn Write,
) -> Result<ProviderRefBoundOutcome, MigrationError> {
    let (target_provider_index, source) = provider_with_index_for_model(model, resolved)?;
    ensure_provider_storage_supported(source)?;
    let source_path = locate_migration_source_path(
        sessions_cfg,
        source,
        &resolved.active_session_id,
        Some(resume_working_dir),
        stderr,
    )?;
    let bytes = read_jsonl_bytes(&source_path)?;
    let slice = match select_recorded_boundary_tail(
        state,
        source,
        &resolved.active_session_id,
        &source_path,
        &bytes,
    )? {
        BoundaryTailSelection::NoBoundary => {
            warn_provider_ref_no_boundary(stderr, &source.name, &resolved.active_session_id);
            return Ok(ProviderRefBoundOutcome::NoBoundary);
        }
        BoundaryTailSelection::BoundaryNotFound { turn_id } => {
            warn_provider_ref_boundary_not_found(
                stderr,
                &resolved.active_session_id,
                &source_path,
                &turn_id,
            );
            return Ok(ProviderRefBoundOutcome::BoundaryNotFound);
        }
        BoundaryTailSelection::Found {
            located_source_offset: Some(0),
            ..
        } => return Ok(ProviderRefBoundOutcome::AlreadyBounded),
        BoundaryTailSelection::Found { slice, .. } => slice,
    };
    let (target_session_id, target_path) = fresh_provider_ref_target(
        state,
        source,
        &resolved.chain_id,
        resume_working_dir,
        fresh_session_id,
        stderr,
    )?;
    write_jsonl_atomic(&target_path, slice.as_ref())?;
    rotate_provider_ref_chain_segment(state, source, resolved, &target_session_id)?;
    preserve_provider_ref_boundary(
        state,
        source,
        &resolved.active_session_id,
        &target_session_id,
    )?;
    emit_migration_line(stderr, &source.name, &source.name, TransitionReason::Manual)?;

    Ok(ProviderRefBoundOutcome::Rotated(MigratedSegment {
        chain_id: resolved.chain_id.clone(),
        source_provider: source.name.clone(),
        source_session_id: resolved.active_session_id.clone(),
        target_provider: source.name.clone(),
        target_provider_index,
        target_session_id,
        target_jsonl_path: target_path,
        reason: TransitionReason::Manual,
    }))
}

fn provider_with_index_for_model<'a>(
    model: &'a ModelConfig,
    resolved: &ResolvedResume,
) -> Result<(usize, &'a oulipoly_config::ProviderConfig), MigrationError> {
    model
        .providers
        .iter()
        .enumerate()
        .find(|(_, provider)| provider.name == resolved.active_provider)
        .ok_or_else(|| MigrationError::ProviderNotInModelPool {
            provider: resolved.active_provider.clone(),
            model_name: model.name.clone(),
        })
}

fn source_provider_for_model<'a>(
    model: &'a ModelConfig,
    resolved: &ResolvedResume,
) -> Result<&'a oulipoly_config::ProviderConfig, MigrationError> {
    provider_with_index_for_model(model, resolved).map(|(_, provider)| provider)
}

fn target_provider_for_index(
    model: &ModelConfig,
    target_provider_index: usize,
) -> Result<&oulipoly_config::ProviderConfig, MigrationError> {
    model.providers.get(target_provider_index).ok_or_else(|| {
        MigrationError::ProviderNotInModelPool {
            provider: target_provider_index.to_string(),
            model_name: model.name.clone(),
        }
    })
}

fn require_provider_resume(
    provider: &oulipoly_config::ProviderConfig,
) -> Result<(), MigrationError> {
    if provider.resume.is_some() {
        Ok(())
    } else {
        Err(MigrationError::ProviderMissingResume {
            provider: provider.name.clone(),
        })
    }
}

fn ensure_migration_storage_supported(
    source: &oulipoly_config::ProviderConfig,
    target: &oulipoly_config::ProviderConfig,
) -> Result<(), MigrationError> {
    ensure_provider_storage_supported(source)?;
    ensure_provider_storage_supported(target)
}

fn ensure_provider_storage_supported(
    provider: &oulipoly_config::ProviderConfig,
) -> Result<(), MigrationError> {
    if provider_storage_class(provider) == Some(ScriptSessionStorageType::CodexSession) {
        Err(MigrationError::CodexMigrationDeferred {
            provider: provider.name.clone(),
        })
    } else {
        Ok(())
    }
}

fn locate_migration_source_path(
    sessions_cfg: &SessionsConfig,
    source: &oulipoly_config::ProviderConfig,
    session_id: &str,
    preferred_working_dir: Option<&Path>,
    stderr: &mut dyn Write,
) -> Result<PathBuf, MigrationError> {
    if let Some(source_path) =
        locate_transcript(sessions_cfg, &source.name, session_id).map_err(|message| {
            MigrationError::TranscriptLocatorFailed {
                provider: source.name.clone(),
                message,
            }
        })?
    {
        return validate_source_path(source, session_id, source_path);
    }
    if let Some(source_path) = preferred_working_dir
        .map(|cwd| find_provider_source_from_storage_cwd(source, session_id, cwd, stderr))
        .transpose()?
        .flatten()
    {
        return validate_source_path(source, session_id, source_path);
    }
    let source_path = find_claude_source_from_storage(source, session_id).ok_or_else(|| {
        MigrationError::SourceMissingStorage {
            provider: source.name.clone(),
        }
    })?;
    validate_source_path(source, session_id, source_path)
}

fn find_provider_source_from_storage_cwd(
    provider: &oulipoly_config::ProviderConfig,
    session_id: &str,
    cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<Option<PathBuf>, MigrationError> {
    let Some(projects_dir) = claude_projects_dir_from_provider_storage(provider) else {
        return Ok(None);
    };
    let project_dir = claude_project_dir_for(&provider.name, cwd, stderr)?;
    let candidate = projects_dir
        .join(project_dir)
        .join(format!("{session_id}.jsonl"));
    Ok(candidate.exists().then_some(candidate))
}

fn validate_source_path(
    source: &oulipoly_config::ProviderConfig,
    session_id: &str,
    source_path: PathBuf,
) -> Result<PathBuf, MigrationError> {
    if !source_path.is_absolute() {
        return Err(MigrationError::SourcePathMalformed {
            provider: source.name.clone(),
            path: source_path.display().to_string(),
        });
    }
    if !source_path.exists() {
        return Err(MigrationError::SourceMissing {
            provider: source.name.clone(),
            session_id: session_id.to_string(),
        });
    }
    Ok(source_path)
}

fn target_jsonl_path(
    target: &oulipoly_config::ProviderConfig,
    resume_working_dir: &Path,
    target_session_id: &str,
    stderr: &mut dyn Write,
) -> Result<PathBuf, MigrationError> {
    let projects_dir = claude_projects_dir_from_provider_storage(target).ok_or_else(|| {
        MigrationError::TargetMissingStorage {
            provider: target.name.clone(),
        }
    })?;
    let cwd_project_dir = claude_project_dir_for(&target.name, resume_working_dir, stderr)?;
    Ok(projects_dir
        .join(cwd_project_dir)
        .join(format!("{target_session_id}.jsonl")))
}

fn read_jsonl_bytes(path: &Path) -> Result<Vec<u8>, MigrationError> {
    std::fs::read(path).map_err(|e| MigrationError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })
}

fn explicit_migration_slice<'a>(
    state: &StateDb,
    source: &oulipoly_config::ProviderConfig,
    session_id: &str,
    source_path: &Path,
    bytes: &'a [u8],
    stderr: &mut dyn Write,
) -> Result<Cow<'a, [u8]>, MigrationError> {
    match select_recorded_boundary_tail(state, source, session_id, source_path, bytes)? {
        BoundaryTailSelection::NoBoundary => Ok(Cow::Borrowed(bytes)),
        BoundaryTailSelection::BoundaryNotFound { turn_id } => {
            warn_explicit_boundary_not_found(stderr, session_id, source_path, &turn_id);
            Ok(Cow::Borrowed(bytes))
        }
        BoundaryTailSelection::Found { slice, .. } => Ok(slice),
    }
}

fn select_recorded_boundary_tail<'a>(
    state: &StateDb,
    source: &oulipoly_config::ProviderConfig,
    session_id: &str,
    source_path: &Path,
    bytes: &'a [u8],
) -> Result<BoundaryTailSelection<'a>, MigrationError> {
    let Some((turn_id, _)) = state
        .latest_compaction_boundary(&source.name, session_id)
        .map_err(|message| MigrationError::Db { message })?
    else {
        return Ok(BoundaryTailSelection::NoBoundary);
    };
    if let Some(offset) = find_turn_offset(bytes, &turn_id) {
        return Ok(BoundaryTailSelection::Found {
            located_source_offset: Some(offset),
            slice: Cow::Borrowed(&bytes[offset..]),
        });
    }
    if let Some(alternate) =
        find_alternate_jsonl_with_boundary(source, session_id, source_path, &turn_id)
    {
        return Ok(BoundaryTailSelection::Found {
            located_source_offset: None,
            slice: Cow::Owned(alternate),
        });
    }
    Ok(BoundaryTailSelection::BoundaryNotFound { turn_id })
}

fn warn_explicit_boundary_not_found(
    stderr: &mut dyn Write,
    session_id: &str,
    source_path: &Path,
    turn_id: &str,
) {
    let _ = writeln!(
        stderr,
        "Warning: recorded compaction boundary turn_id={turn_id} not found in any candidate JSONL for session_id={session_id} located_path={path}; falling back to full source slice",
        path = source_path.display(),
    );
}

fn warn_provider_ref_no_boundary(stderr: &mut dyn Write, provider: &str, session_id: &str) {
    let _ = writeln!(
        stderr,
        "Warning: no recorded compaction boundary for provider={provider} session_id={session_id}; using original session id"
    );
}

fn warn_provider_ref_boundary_not_found(
    stderr: &mut dyn Write,
    session_id: &str,
    source_path: &Path,
    turn_id: &str,
) {
    let _ = writeln!(
        stderr,
        "Warning: recorded compaction boundary turn_id={turn_id} not found in any candidate JSONL for session_id={session_id} located_path={path}; using original session id",
        path = source_path.display(),
    );
}

fn ensure_no_conflicting_active_segment(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    own_chain_id: &str,
) -> Result<(), MigrationError> {
    if let Some(conflicting_chain_id) = state
        .find_conflicting_active_segment(provider_name, session_id, own_chain_id)
        .map_err(|message| MigrationError::Db { message })?
    {
        return Err(MigrationError::TargetSessionInUseByOtherChain {
            provider: provider_name.to_string(),
            session_id: session_id.to_string(),
            conflicting_chain_id,
        });
    }
    Ok(())
}

fn write_jsonl_atomic(target_path: &Path, bytes: &[u8]) -> Result<(), MigrationError> {
    let target_dir =
        target_path
            .parent()
            .ok_or_else(|| MigrationError::TargetDirectoryCreateFailed {
                path: target_path.display().to_string(),
                message: "target path has no parent directory".to_string(),
            })?;
    std::fs::create_dir_all(target_dir).map_err(|e| {
        MigrationError::TargetDirectoryCreateFailed {
            path: target_dir.display().to_string(),
            message: e.to_string(),
        }
    })?;
    let tmp = target_path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| MigrationError::Io {
        path: tmp.display().to_string(),
        message: e.to_string(),
    })?;
    std::fs::rename(&tmp, target_path).map_err(|e| MigrationError::Io {
        path: target_path.display().to_string(),
        message: e.to_string(),
    })
}

fn fresh_provider_ref_target(
    state: &StateDb,
    provider: &oulipoly_config::ProviderConfig,
    chain_id: &str,
    resume_working_dir: &Path,
    fresh_session_id: &mut dyn FnMut() -> String,
    stderr: &mut dyn Write,
) -> Result<(String, PathBuf), MigrationError> {
    let mut last_error = None;
    for _ in 0..MAX_FRESH_SESSION_ID_ATTEMPTS {
        let candidate = fresh_session_id();
        let target_path = target_jsonl_path(provider, resume_working_dir, &candidate, stderr)?;
        if target_path.exists() {
            last_error = Some(MigrationError::TargetAlreadyExists {
                provider: provider.name.clone(),
                path: target_path.display().to_string(),
            });
            continue;
        }
        match ensure_no_conflicting_active_segment(state, &provider.name, &candidate, chain_id) {
            Ok(()) => return Ok((candidate, target_path)),
            Err(err @ MigrationError::TargetSessionInUseByOtherChain { .. }) => {
                last_error = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    Err(
        last_error.unwrap_or_else(|| MigrationError::TargetAlreadyExists {
            provider: provider.name.clone(),
            path: resume_working_dir.display().to_string(),
        }),
    )
}

fn rotate_provider_ref_chain_segment(
    state: &StateDb,
    source: &oulipoly_config::ProviderConfig,
    resolved: &ResolvedResume,
    target_session_id: &str,
) -> Result<(), MigrationError> {
    let now = chrono::Utc::now();
    state
        .rotate_chain_segment_transactionally(ChainSegmentRotationInput {
            chain_id: &resolved.chain_id,
            source_provider_name: &source.name,
            source_session_id: &resolved.active_session_id,
            target_provider_name: &source.name,
            target_session_id,
            changed_at: &now,
            reason: TransitionReason::Manual,
        })
        .map(|_| ())
        .map_err(|message| MigrationError::Db { message })
}

fn preserve_provider_ref_boundary(
    state: &StateDb,
    source: &oulipoly_config::ProviderConfig,
    source_session_id: &str,
    target_session_id: &str,
) -> Result<(), MigrationError> {
    let preserved = state
        .preserve_compaction_boundary_for_session(
            &source.name,
            source_session_id,
            &source.name,
            target_session_id,
        )
        .map_err(|message| MigrationError::Db { message })?;
    if preserved {
        Ok(())
    } else {
        Err(MigrationError::Db {
            message: "compaction boundary disappeared before preservation".to_string(),
        })
    }
}

fn emit_migration_line(
    stderr: &mut dyn Write,
    source_provider: &str,
    target_provider: &str,
    reason: TransitionReason,
) -> Result<(), MigrationError> {
    writeln!(
        stderr,
        "[migrate] {} -> {} reason={}",
        source_provider,
        target_provider,
        reason.as_str()
    )
    .map_err(|e| MigrationError::Io {
        path: "<stderr>".to_string(),
        message: e.to_string(),
    })
}

pub(crate) fn claude_project_dir_for(
    provider: &str,
    cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<String, MigrationError> {
    if cwd.as_os_str().is_empty() {
        return Err(MigrationError::SpawnCwdUnsupported {
            provider: provider.to_string(),
            cwd: cwd.display().to_string(),
        });
    }

    let path_for_hash = match std::fs::canonicalize(cwd) {
        Ok(resolved) => resolved,
        Err(error) => {
            let _ = writeln!(
                stderr,
                "Warning: Claude project-dir canonicalize failed for provider={provider} cwd={} error={}; falling back to literal cwd",
                cwd.display(),
                error
            );
            cwd.to_path_buf()
        }
    };

    let input = path_for_hash.to_string_lossy();
    let mut encoded = String::with_capacity(input.len());
    for ch in input.chars() {
        let mapped = match ch {
            '/' | '\\' => '-',
            c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
            _ => '-',
        };
        encoded.push(mapped);
    }
    Ok(encoded)
}

pub fn find_claude_source_from_storage(
    provider: &oulipoly_config::ProviderConfig,
    session_id: &str,
) -> Option<PathBuf> {
    let projects_dir = claude_projects_dir_from_provider_storage(provider)?;
    let entries = std::fs::read_dir(projects_dir).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn provider_storage_class(
    provider: &oulipoly_config::ProviderConfig,
) -> Option<ScriptSessionStorageType> {
    provider
        .session_storage
        .as_ref()
        .and_then(SessionStorage::script_storage_type)
}

pub(crate) fn provider_has_derivable_claude_projects_dir(
    provider: &oulipoly_config::ProviderConfig,
) -> bool {
    claude_projects_dir_from_provider_storage(provider).is_some()
}

fn claude_projects_dir_from_provider_storage(
    provider: &oulipoly_config::ProviderConfig,
) -> Option<PathBuf> {
    claude_projects_dir_from_storage(provider.session_storage.as_ref()?)
}

fn claude_projects_dir_from_storage(storage: &SessionStorage) -> Option<PathBuf> {
    match storage {
        SessionStorage::ClaudeCode { projects_dir } => Some(projects_dir.clone()),
        SessionStorage::Script {
            cwd_script,
            transcript_script,
            storage_type: Some(ScriptSessionStorageType::ClaudeCode),
        } => transcript_script
            .as_deref()
            .and_then(claude_projects_dir_from_adapter_command)
            .or_else(|| claude_projects_dir_from_adapter_command(cwd_script)),
        _ => None,
    }
}

fn claude_projects_dir_from_adapter_command(command: &str) -> Option<PathBuf> {
    let parts = crate::executor::cli::shell_split(command);
    let adapter = parts.first()?;
    let storage_root = parts.get(1)?;
    let adapter_name = Path::new(adapter).file_name()?.to_str()?;
    match adapter_name {
        "claude-code-cwd" | "claude-code-locate-transcript" | "claude-code-turns" => {
            Some(expand_leading_tilde(PathBuf::from(storage_root)))
        }
        _ => None,
    }
}

fn expand_leading_tilde(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    let Some(home) = dirs::home_dir() else {
        return path;
    };
    if raw == "~" {
        return home;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    path
}

fn find_alternate_jsonl_with_boundary(
    provider: &oulipoly_config::ProviderConfig,
    session_id: &str,
    located_path: &Path,
    turn_id: &str,
) -> Option<Vec<u8>> {
    let projects_dir = claude_projects_dir_from_provider_storage(provider)?;
    let entries = std::fs::read_dir(&projects_dir).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if !candidate.is_file() {
            continue;
        }
        if candidate == located_path {
            continue;
        }
        let Ok(bytes) = std::fs::read(&candidate) else {
            continue;
        };
        if let Some(offset) = find_turn_offset(&bytes, turn_id) {
            return Some(bytes[offset..].to_vec());
        }
    }
    None
}

fn find_turn_offset(buf: &[u8], turn_id: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in buf.split_inclusive(|byte| *byte == b'\n') {
        if jsonl_line_turn_id_matches(trim_jsonl_newline(line), turn_id) {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn trim_jsonl_newline(line: &[u8]) -> &[u8] {
    let without_lf = line.strip_suffix(b"\n").unwrap_or(line);
    without_lf.strip_suffix(b"\r").unwrap_or(without_lf)
}

fn jsonl_line_turn_id_matches(line: &[u8], turn_id: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(line)
        .ok()
        .is_some_and(|value| json_turn_id_field_matches(&value, turn_id))
}

fn json_turn_id_field_matches(value: &serde_json::Value, turn_id: &str) -> bool {
    ["uuid", "turn_id", "turnId", "id"]
        .into_iter()
        .any(|field| value.get(field).and_then(|raw| raw.as_str()) == Some(turn_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::{
        ModelConfig, ProviderConfig, ResumeKind, ResumeStrategy, SessionsConfig,
    };
    use oulipoly_state::{InvocationStart, ResolvedResume, StateDb};

    fn model_with_storage(
        source_projects: &std::path::Path,
        target_projects: &std::path::Path,
    ) -> ModelConfig {
        let provider = |name: &str, projects_dir: PathBuf| ProviderConfig {
            name: name.to_string(),
            command: name.to_string(),
            args: Vec::new(),
            interactive_args: Some(vec!["launch".to_string()]),
            resume: Some(ResumeStrategy {
                kind: ResumeKind::Flag,
                flag: Some("--resume".to_string()),
                subcommand: None,
            }),
            session_capture: None,
            resume_acceptance: None,
            session_storage: Some(SessionStorage::ClaudeCode { projects_dir }),
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: oulipoly_config::PromptMode::Arg,
            providers: vec![
                provider("claude", source_projects.to_path_buf()),
                provider("claude2", target_projects.to_path_buf()),
            ],
            inputs: Vec::new(),
            provider: None,
        }
    }

    fn model_with_script_storage(
        source_projects: &std::path::Path,
        target_projects: &std::path::Path,
    ) -> ModelConfig {
        let provider = |name: &str, projects_dir: &std::path::Path| ProviderConfig {
            name: name.to_string(),
            command: name.to_string(),
            args: Vec::new(),
            interactive_args: Some(vec!["launch".to_string()]),
            resume: Some(ResumeStrategy {
                kind: ResumeKind::Flag,
                flag: Some("--resume".to_string()),
                subcommand: None,
            }),
            session_capture: None,
            resume_acceptance: None,
            session_storage: Some(SessionStorage::Script {
                cwd_script: format!("claude-code-cwd {}", projects_dir.display()),
                transcript_script: Some(format!(
                    "claude-code-locate-transcript {}",
                    projects_dir.display()
                )),
                storage_type: Some(ScriptSessionStorageType::ClaudeCode),
            }),
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        };
        ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: oulipoly_config::PromptMode::Arg,
            providers: vec![
                provider("claude", source_projects),
                provider("claude2", target_projects),
            ],
            inputs: Vec::new(),
            provider: None,
        }
    }

    fn claude_project_dir_name(path: &std::path::Path) -> String {
        path.to_string_lossy()
            .chars()
            .map(|c| match c {
                '/' | '\\' => '-',
                c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
                _ => '-',
            })
            .collect()
    }

    fn seed_source_jsonl(
        source_projects: &std::path::Path,
        source_workspace: &std::path::Path,
        session_id: &str,
    ) -> PathBuf {
        let source_dir = source_projects.join(claude_project_dir_name(source_workspace));
        std::fs::create_dir_all(&source_dir).unwrap();
        let source_path = source_dir.join(format!("{session_id}.jsonl"));
        std::fs::write(
            &source_path,
            format!(
                r#"{{"uuid":"turn-1","sessionId":"{session_id}","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
        source_path
    }

    fn seed_resolved(
        state: &StateDb,
        model: &ModelConfig,
        session_id: &str,
    ) -> (ResolvedResume, String) {
        let invocation_id = state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid::Uuid::new_v4().to_string(),
                model_name: "claude-opus".to_string(),
                provider_name: "claude".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        state
            .update_session_capture(invocation_id, Some(session_id), "fixture")
            .unwrap();
        state
            .mint_chain_for_invocation_session(invocation_id)
            .unwrap();
        let chain_id = state
            .chain_id_for_segment("claude", session_id)
            .unwrap()
            .unwrap();
        (
            ResolvedResume {
                chain_id: chain_id.clone(),
                model_name: Some(model.name.clone()),
                model: Some(model.clone()),
                active_provider: "claude".to_string(),
                active_session_id: session_id.to_string(),
            },
            chain_id,
        )
    }

    // risk: Migration mechanic source UUID reuse; level: particular-integration; source: proposal §11.1 Migration mechanic / A1.
    #[test]
    fn migration_reuses_source_session_id_when_source_and_spawn_cwd_match() {
        let dir = tempfile::tempdir().unwrap();
        let source_projects = dir.path().join("source-projects");
        let target_projects = dir.path().join("target-projects");
        let source_workspace = dir.path().join("worktrees").join("same-workspace");
        let session_id = "dd116a3c-6819-42b1-b3d2-f512331eb5ec";
        seed_source_jsonl(&source_projects, &source_workspace, session_id);
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        let model = model_with_storage(&source_projects, &target_projects);
        let (resolved, chain_id) = seed_resolved(&state, &model, session_id);
        let mut stderr = Vec::new();

        let migrated = migrate_chain_segment(
            &state,
            &SessionsConfig::default(),
            &model,
            &resolved,
            &source_workspace,
            1,
            TransitionReason::Manual,
            &mut stderr,
        )
        .unwrap();

        assert_eq!(migrated.target_session_id, session_id);
        assert_eq!(
            migrated.target_jsonl_path,
            target_projects
                .join(claude_project_dir_name(&source_workspace))
                .join(format!("{session_id}.jsonl"))
        );
        assert!(migrated.target_jsonl_path.exists());
        assert_eq!(
            state.chain_id_for_segment("claude2", session_id).unwrap(),
            Some(chain_id)
        );
    }

    // risk: RC-1 cwd/source project dir mismatch; level: particular-integration; source: ~/projects/agent-runner/planning/trunk/research/14-session-migration-rca.md (RC-1) + ~/projects/agent-runner/planning/trunk/research/14-problem-map.md §2.
    #[test]
    fn migration_writes_target_under_spawn_cwd_when_source_and_spawn_cwd_differ() {
        let dir = tempfile::tempdir().unwrap();
        let source_projects = dir.path().join("source-projects");
        let target_projects = dir.path().join("target-projects");
        let source_workspace = dir.path().join("worktrees").join("source-workspace");
        let resume_workspace = dir.path().join("worktrees").join("resume-workspace");
        let session_id = "dd116a3c-6819-42b1-b3d2-f512331eb5ec";
        seed_source_jsonl(&source_projects, &source_workspace, session_id);
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        let model = model_with_storage(&source_projects, &target_projects);
        let (resolved, chain_id) = seed_resolved(&state, &model, session_id);
        let mut stderr = Vec::new();

        let migrated = migrate_chain_segment(
            &state,
            &SessionsConfig::default(),
            &model,
            &resolved,
            &resume_workspace,
            1,
            TransitionReason::Manual,
            &mut stderr,
        )
        .unwrap();

        let source_cwd_target = target_projects
            .join(claude_project_dir_name(&source_workspace))
            .join(format!("{session_id}.jsonl"));
        let resume_cwd_target = target_projects
            .join(claude_project_dir_name(&resume_workspace))
            .join(format!("{session_id}.jsonl"));

        assert_eq!(migrated.target_session_id, session_id);
        assert_eq!(migrated.target_jsonl_path, resume_cwd_target);
        assert!(migrated.target_jsonl_path.exists());
        assert!(!source_cwd_target.exists());
        assert_eq!(
            state.chain_id_for_segment("claude2", session_id).unwrap(),
            Some(chain_id)
        );
    }

    #[test]
    fn migration_supports_script_declared_claude_code_storage() {
        let dir = tempfile::tempdir().unwrap();
        let source_projects = dir.path().join("source-projects");
        let target_projects = dir.path().join("target-projects");
        let source_workspace = dir.path().join("worktrees").join("same-workspace");
        let session_id = "dd116a3c-6819-42b1-b3d2-f512331eb5ec";
        seed_source_jsonl(&source_projects, &source_workspace, session_id);
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        let model = model_with_script_storage(&source_projects, &target_projects);
        let (resolved, chain_id) = seed_resolved(&state, &model, session_id);
        let mut stderr = Vec::new();

        let migrated = migrate_chain_segment(
            &state,
            &SessionsConfig::default(),
            &model,
            &resolved,
            &source_workspace,
            1,
            TransitionReason::Manual,
            &mut stderr,
        )
        .unwrap();

        assert_eq!(
            migrated.target_jsonl_path,
            target_projects
                .join(claude_project_dir_name(&source_workspace))
                .join(format!("{session_id}.jsonl"))
        );
        assert!(migrated.target_jsonl_path.exists());
        assert_eq!(
            state.chain_id_for_segment("claude2", session_id).unwrap(),
            Some(chain_id)
        );
    }

    #[test]
    fn claude_projects_dir_from_adapter_command_expands_home_relative_storage_root() {
        let expected = dirs::home_dir().unwrap().join(".claude2/projects");

        let actual =
            claude_projects_dir_from_adapter_command("claude-code-cwd ~/.claude2/projects")
                .unwrap();

        assert_eq!(actual, expected);
    }

    // risk: Cwd-to-project-dir encoding correctness; level: unit; source: proposal §1 helper signature / A2.
    #[test]
    fn claude_project_dir_for_encodes_absolute_unix_path() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work_tree").join("project.v1");
        std::fs::create_dir_all(&cwd).unwrap();
        let expected = claude_project_dir_name(&cwd.canonicalize().unwrap());
        let mut stderr = Vec::new();

        assert_eq!(
            claude_project_dir_for("claude", &cwd, &mut stderr).unwrap(),
            expected
        );
    }

    // risk: Cwd-to-project-dir encoding correctness; level: unit; source: proposal §1 helper signature / A2.
    #[test]
    fn claude_project_dir_for_accepts_relative_non_empty_path() {
        let cwd = std::path::Path::new("relative/work_tree/project.v1");
        let expected = match cwd.canonicalize() {
            Ok(resolved) => claude_project_dir_name(&resolved),
            Err(_) => claude_project_dir_name(cwd),
        };
        let mut stderr = Vec::new();

        assert_eq!(
            claude_project_dir_for("claude", cwd, &mut stderr).unwrap(),
            expected
        );
    }

    // risk: Cwd-to-project-dir encoding correctness; level: unit; source: proposal §1 helper signature / A2.
    #[test]
    fn claude_project_dir_for_rejects_empty_path() {
        let mut stderr = Vec::new();
        let err = claude_project_dir_for("claude", std::path::Path::new(""), &mut stderr)
            .expect_err("empty cwd should be rejected");

        assert!(matches!(
            err,
            MigrationError::SpawnCwdUnsupported { provider, cwd }
                if provider == "claude" && cwd.is_empty()
        ));
    }

    // risk: Canonicalize failure silently hashes the wrong cwd; level: unit; source: contract §4 invariant 3.
    #[test]
    fn claude_project_dir_for_warns_and_uses_literal_path_when_canonicalize_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("missing_work_tree").join("project.v1");
        let expected = claude_project_dir_name(&cwd);
        let mut stderr = Vec::new();

        assert_eq!(
            claude_project_dir_for("claude-target", &cwd, &mut stderr).unwrap(),
            expected
        );

        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("Warning:"));
        assert!(stderr.contains("provider=claude-target"));
        assert!(stderr.contains(&format!("cwd={}", cwd.display())));
        assert!(stderr.contains("falling back to literal cwd"));
    }
}
