use crate::balancer::TransitionReason;
use crate::sessions::locate_transcript;
use oulipoly_config::{ModelConfig, ScriptSessionStorageType, SessionStorage, SessionsConfig};
use oulipoly_state::{ResolvedResume, StateDb};
use std::io::Write;
use std::path::{Path, PathBuf};

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
    if provider_storage_class(source) == Some(ScriptSessionStorageType::CodexSession)
        || provider_storage_class(target) == Some(ScriptSessionStorageType::CodexSession)
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
    let projects_dir = claude_projects_dir_from_provider_storage(target).ok_or_else(|| {
        MigrationError::TargetMissingStorage {
            provider: target.name.clone(),
        }
    })?;
    let cwd_project_dir = claude_project_dir_for(&target.name, resume_working_dir, stderr)?;
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

    let target_dir = projects_dir.join(&cwd_project_dir);
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
