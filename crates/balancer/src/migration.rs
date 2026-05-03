use agent_runner_config::{ModelConfig, SessionStorage, SessionsConfig};
use agent_runner_state::TransitionReason;
use agent_runner_state::{ResolvedResume, StateDb};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// TODO(post-C): dedupe with agent_runner_session::scan::SCRIPT_TIMEOUT_SECS.
// Cannot import from agent-runner-session here (would create balancer->session edge).
// Pre-C value: 600.
const SCRIPT_TIMEOUT_SECS: u64 = 600;

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
        .iter()
        .find(|provider| provider.name == resolved.active_provider)
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

    let target_session_id = resolved.active_session_id.clone();
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
    if let Some(conflicting_chain_id) = state
        .find_conflicting_active_segment(&target.name, &target_session_id, &resolved.chain_id)
        .map_err(|message| MigrationError::Db { message })?
    {
        return Err(MigrationError::TargetSessionInUseByOtherChain {
            provider: target.name.clone(),
            session_id: target_session_id.clone(),
            conflicting_chain_id,
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

pub fn find_claude_source_from_storage(
    provider: &agent_runner_config::ProviderConfig,
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

fn locate_transcript(
    sessions_cfg: &SessionsConfig,
    provider_name: &str,
    session_id: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(entry) = sessions_cfg.get(provider_name) else {
        return Ok(None);
    };
    let Some(locator) = entry.transcript_locator.as_deref() else {
        return Ok(None);
    };

    let state_dir = if let Some(dir) = &entry.state_dir {
        dir.clone()
    } else {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oulipoly-agent-runner")
            .join("sessions")
            .join(provider_name)
    };
    std::fs::create_dir_all(&state_dir)
        .map_err(|e| format!("could not create state_dir {}: {e}", state_dir.display()))?;

    let stdout =
        run_session_script_os(locator, &state_dir, Some(session_id), "transcript locator")?;
    transcript_locator_output(stdout)
}

fn transcript_locator_output(stdout: String) -> Result<Option<PathBuf>, String> {
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    match lines.as_slice() {
        [] => Err("transcript locator returned empty stdout".to_string()),
        [line] => Ok(Some(PathBuf::from(line))),
        _ => Err("transcript locator stdout was not a single line".to_string()),
    }
}

fn run_session_script_os(
    script: &str,
    state_dir: &std::path::Path,
    session_id: Option<&str>,
    script_kind: &str,
) -> Result<String, String> {
    use std::io::Read;

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd.env("STATE_DIR", state_dir);
    if let Some(session_id) = session_id {
        cmd.env("SESSION_ID", session_id);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn {script_kind}: {e}"))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("Failed to wait for {script_kind}: {e}"))?
        {
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_string(&mut stdout);
            }
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            if !status.success() {
                return Err(format!(
                    "{} exited {}: {}",
                    capitalize_script_kind(script_kind),
                    status.code().unwrap_or(-1),
                    stderr.trim()
                ));
            }
            return Ok(stdout);
        }
        if start.elapsed() > Duration::from_secs(SCRIPT_TIMEOUT_SECS) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{script_kind} timed out"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn capitalize_script_kind(script_kind: &str) -> String {
    let mut chars = script_kind.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Script".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runner_config::{
        ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionsConfig,
    };
    use agent_runner_state::{InvocationStart, StateDb};

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
        };
        ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                provider("claude", source_projects.to_path_buf()),
                provider("claude2", target_projects.to_path_buf()),
            ],
            inputs: Vec::new(),
        }
    }

    // risk: Migration mechanic source UUID reuse; level: particular-integration; source: proposal §11.1 Migration mechanic / A1.
    #[test]
    fn migration_reuses_source_session_id_on_target_side() {
        let dir = tempfile::tempdir().unwrap();
        let source_projects = dir.path().join("source-projects");
        let target_projects = dir.path().join("target-projects");
        let cwd_hash = "cwd-hash";
        let session_id = "dd116a3c-6819-42b1-b3d2-f512331eb5ec";
        let source_dir = source_projects.join(cwd_hash);
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(
            source_dir.join(format!("{session_id}.jsonl")),
            format!(
                r#"{{"uuid":"turn-1","sessionId":"{session_id}","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
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
        let model = model_with_storage(&source_projects, &target_projects);
        let resolved = ResolvedResume {
            chain_id: chain_id.clone(),
            model_name: Some(model.name.clone()),
            model: Some(model.clone()),
            active_provider: "claude".to_string(),
            active_session_id: session_id.to_string(),
        };
        let mut stderr = Vec::new();

        let migrated = migrate_chain_segment(
            &state,
            &SessionsConfig::default(),
            &model,
            &resolved,
            1,
            TransitionReason::Manual,
            &mut stderr,
        )
        .unwrap();

        assert_eq!(migrated.target_session_id, session_id);
        assert_eq!(
            migrated.target_jsonl_path,
            target_projects
                .join(cwd_hash)
                .join(format!("{session_id}.jsonl"))
        );
        assert!(migrated.target_jsonl_path.exists());
        assert_eq!(
            state.chain_id_for_segment("claude2", session_id).unwrap(),
            Some(chain_id)
        );
    }
}
