use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig,
};
use oulipoly_state::{InvocationStart, ModelStore, ResolvedResume, StateDb};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub mod rc1_cwd_project_dir_mismatch;

pub const SESSION_ID: &str = "1bc948a0-2c57-4261-b703-7e4c27ecff00";

pub struct MigrationFixture {
    pub dir: tempfile::TempDir,
    pub source_projects: PathBuf,
    pub target_projects: PathBuf,
    pub source_workspace: PathBuf,
    pub resume_workspace: PathBuf,
}

impl MigrationFixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source_projects = dir.path().join("claude3").join("projects");
        let target_projects = dir.path().join("claude").join("projects");
        let source_workspace = dir.path().join("worktrees").join("source-workspace");
        let resume_workspace = dir.path().join("worktrees").join("resume-workspace");
        fs::create_dir_all(&source_workspace).unwrap();
        fs::create_dir_all(&resume_workspace).unwrap();
        Self {
            dir,
            source_projects,
            target_projects,
            source_workspace,
            resume_workspace,
        }
    }

    pub fn state(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    pub fn model(&self, target_command: &Path) -> ModelConfig {
        ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                claude_provider("claude3", Path::new("claude3"), &self.source_projects),
                claude_provider("claude", target_command, &self.target_projects),
            ],
            inputs: Vec::new(),
        }
    }

    pub fn seed_source_jsonl(&self) -> PathBuf {
        let source_dir = self
            .source_projects
            .join(claude_project_dir_name(&self.source_workspace));
        fs::create_dir_all(&source_dir).unwrap();
        let path = source_dir.join(format!("{SESSION_ID}.jsonl"));
        fs::write(
            &path,
            format!(
                "{{\"sessionId\":\"{SESSION_ID}\",\"type\":\"summary\",\"timestamp\":\"2026-04-17T08:00:00Z\"}}\n\
                 {{\"sessionId\":\"{SESSION_ID}\",\"uuid\":\"turn-1\",\"type\":\"assistant\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"cwd\":\"{}\"}}\n",
                self.source_workspace.display()
            ),
        )
        .unwrap();
        path
    }

    pub fn seed_chain(&self, db: &StateDb, model: &ModelConfig) -> ResolvedResume {
        let invocation_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid::Uuid::new_v4().to_string(),
                model_name: model.name.clone(),
                provider_name: "claude3".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.update_session_capture(invocation_id, Some(SESSION_ID), "fixture")
            .unwrap();
        db.mint_chain_for_invocation_session(invocation_id).unwrap();
        let models: ModelStore = HashMap::from([(model.name.clone(), model.clone())]);
        db.resolve_resume(&models, SESSION_ID, Some(&model.name))
            .unwrap()
    }

    pub fn fake_claude(&self) -> PathBuf {
        let script = self.dir.path().join("fake-claude.sh");
        fs::write(
            &script,
            format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
sid=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--resume" ]; then
    shift
    sid="${{1:-}}"
    break
  fi
  shift
done
project=$(printf '%s' "$PWD" | sed -e 's#[/\\]#-#g' -e 's/[^A-Za-z0-9-]/-/g')
candidate="{}/$project/$sid.jsonl"
if [ -n "$sid" ] && [ -f "$candidate" ]; then
  printf '{{"session_id":"%s","status":"resumed"}}\n' "$sid"
  exit 0
fi
printf 'No conversation found with session ID: %s\n' "$sid" >&2
exit 1
"#,
                self.target_projects.display()
            ),
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        script
    }
}

pub fn claude_project_dir_name(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
            _ => '-',
        })
        .collect()
}

fn claude_provider(name: &str, command: &Path, projects_dir: &Path) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        command: command.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: Some(Vec::new()),
        resume: Some(ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }),
        session_capture: None,
        resume_acceptance: None,
        session_storage: Some(SessionStorage::ClaudeCode {
            projects_dir: projects_dir.to_path_buf(),
        }),
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

pub fn empty_sessions_config() -> SessionsConfig {
    SessionsConfig::default()
}
