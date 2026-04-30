use crate::balancer::TransitionReason;
use crate::config::{ModelConfig, SessionStorage, SessionsConfig};
use crate::sessions::locate_transcript;
use crate::state::{ResolvedResume, StateDb};
use std::io::Write;
use std::path::PathBuf;
use uuid::Uuid;

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
    TargetAlreadyExists {
        provider: String,
        path: String,
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

pub fn migrate_chain_segment(
    state: &StateDb,
    sessions_cfg: &SessionsConfig,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    target_provider_index: usize,
    reason: TransitionReason,
    stderr: &mut dyn Write,
) -> Result<MigratedSegment, MigrationError> {
    let source = model
        .providers
        .get(resolved.active_provider_index)
        .ok_or_else(|| MigrationError::ProviderNotInModelPool {
            provider: resolved.active_provider.clone(),
            model_name: model.name.clone(),
        })?;
    let target = model.providers.get(target_provider_index).ok_or_else(|| {
        MigrationError::ProviderNotInModelPool {
            provider: target_provider_index.to_string(),
            model_name: model.name.clone(),
        }
    })?;
    if source.resume.is_none() {
        return Err(MigrationError::ProviderMissingResume {
            provider: source.name.clone(),
        });
    }
    if target.resume.is_none() {
        return Err(MigrationError::ProviderMissingResume {
            provider: target.name.clone(),
        });
    }
    if matches!(source.session_storage, Some(SessionStorage::Codex { .. }))
        || matches!(target.session_storage, Some(SessionStorage::Codex { .. }))
    {
        return Err(MigrationError::CodexMigrationDeferred {
            provider: source.name.clone(),
        });
    }

    let source_path = locate_transcript(sessions_cfg, &source.name, &resolved.active_session_id)
        .map_err(|message| MigrationError::TranscriptLocatorFailed {
            provider: source.name.clone(),
            message,
        })?
        .or_else(|| find_claude_source_from_storage(source, &resolved.active_session_id))
        .ok_or_else(|| MigrationError::SourceMissingStorage {
            provider: source.name.clone(),
        })?;
    if !source_path.is_absolute() {
        return Err(MigrationError::SourcePathMalformed {
            provider: source.name.clone(),
            path: source_path.display().to_string(),
        });
    }
    if !source_path.exists() {
        return Err(MigrationError::SourceMissing {
            provider: source.name.clone(),
            session_id: resolved.active_session_id.clone(),
        });
    }

    let target_session_id = Uuid::new_v4().to_string();
    let SessionStorage::ClaudeCode { projects_dir } =
        target
            .session_storage
            .as_ref()
            .ok_or_else(|| MigrationError::TargetMissingStorage {
                provider: target.name.clone(),
            })?
    else {
        return Err(MigrationError::CodexMigrationDeferred {
            provider: target.name.clone(),
        });
    };
    let cwd_hash = source_path
        .parent()
        .and_then(|p| p.file_name())
        .ok_or_else(|| MigrationError::SourcePathMalformed {
            provider: source.name.clone(),
            path: source_path.display().to_string(),
        })?;
    let bytes = std::fs::read(&source_path).map_err(|e| MigrationError::Io {
        path: source_path.display().to_string(),
        message: e.to_string(),
    })?;
    let slice = if let Some((turn_id, _)) = state
        .latest_compaction_boundary(&source.name, &resolved.active_session_id)
        .map_err(|message| MigrationError::Db { message })?
    {
        let text = String::from_utf8_lossy(&bytes);
        let offset = text
            .lines()
            .scan(0usize, |offset, line| {
                let start = *offset;
                *offset += line.len() + 1;
                Some((start, line))
            })
            .find_map(|(start, line)| line.contains(&turn_id).then_some(start))
            .ok_or_else(|| MigrationError::CompactionBoundaryNotInJsonl {
                session_id: resolved.active_session_id.clone(),
                turn_id,
            })?;
        &bytes[offset..]
    } else {
        &bytes[..]
    };

    let target_dir = projects_dir.join(cwd_hash);
    std::fs::create_dir_all(&target_dir).map_err(|e| {
        MigrationError::TargetDirectoryCreateFailed {
            path: target_dir.display().to_string(),
            message: e.to_string(),
        }
    })?;
    let target_path = target_dir.join(format!("{target_session_id}.jsonl"));
    if target_path.exists() {
        return Err(MigrationError::TargetAlreadyExists {
            provider: target.name.clone(),
            path: target_path.display().to_string(),
        });
    }
    let tmp = target_path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, slice).map_err(|e| MigrationError::Io {
        path: tmp.display().to_string(),
        message: e.to_string(),
    })?;
    std::fs::rename(&tmp, &target_path).map_err(|e| MigrationError::Io {
        path: target_path.display().to_string(),
        message: e.to_string(),
    })?;

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

fn find_claude_source_from_storage(
    provider: &crate::config::ProviderConfig,
    session_id: &str,
) -> Option<PathBuf> {
    let SessionStorage::ClaudeCode { projects_dir } = provider.session_storage.as_ref()? else {
        return None;
    };
    let entries = std::fs::read_dir(projects_dir).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}
