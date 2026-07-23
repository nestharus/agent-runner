//! Declared roles: accessor, formatter, mapper, orchestration.

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

#[cfg(test)]
pub mod age158_characterization;

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
        let paths = migration_fixture_paths();
        create_migration_workspaces(&paths);
        assemble_migration_fixture(paths)
    }

    pub fn state(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    pub fn model(&self, target_command: &Path) -> ModelConfig {
        assemble_migration_model(&self.source_projects, target_command, &self.target_projects)
    }

    pub fn seed_source_jsonl(&self) -> PathBuf {
        let path = source_jsonl_path(&self.source_projects, &self.source_workspace);
        write_source_jsonl(&path, &self.source_workspace);
        path
    }

    pub fn seed_chain(&self, db: &StateDb, model: &ModelConfig) -> ResolvedResume {
        let invocation_id = start_source_invocation(db, model);
        capture_source_session(db, invocation_id);
        mint_source_chain(db, invocation_id);
        let models = model_store_for_fixture(model);
        resolve_seeded_resume(db, &models, model)
    }

    pub fn fake_claude(&self) -> PathBuf {
        let script = fake_claude_path(&self.dir);
        fs::write(&script, fake_claude_script(&self.target_projects)).unwrap();
        mark_fake_provider_executable(&script);
        script
    }
}

struct MigrationFixturePaths {
    dir: tempfile::TempDir,
    source_projects: PathBuf,
    target_projects: PathBuf,
    source_workspace: PathBuf,
    resume_workspace: PathBuf,
}

fn migration_fixture_paths() -> MigrationFixturePaths {
    let dir = tempfile::tempdir().unwrap();
    let source_projects = dir.path().join("claude3").join("projects");
    let target_projects = dir.path().join("claude").join("projects");
    let source_workspace = dir.path().join("worktrees").join("source-workspace");
    let resume_workspace = dir.path().join("worktrees").join("resume-workspace");

    MigrationFixturePaths {
        dir,
        source_projects,
        target_projects,
        source_workspace,
        resume_workspace,
    }
}

fn create_migration_workspaces(paths: &MigrationFixturePaths) {
    fs::create_dir_all(&paths.source_workspace).unwrap();
    fs::create_dir_all(&paths.resume_workspace).unwrap();
}

fn assemble_migration_fixture(paths: MigrationFixturePaths) -> MigrationFixture {
    MigrationFixture {
        dir: paths.dir,
        source_projects: paths.source_projects,
        target_projects: paths.target_projects,
        source_workspace: paths.source_workspace,
        resume_workspace: paths.resume_workspace,
    }
}

fn assemble_migration_model(
    source_projects: &Path,
    target_command: &Path,
    target_projects: &Path,
) -> ModelConfig {
    ModelConfig {
        name: "claude-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            source_claude_provider(source_projects),
            target_claude_provider(target_command, target_projects),
        ],
        inputs: Vec::new(),
        provider: None,
    }
}

fn source_claude_provider(source_projects: &Path) -> ProviderConfig {
    claude_provider("claude3", Path::new("claude3"), source_projects)
}

fn target_claude_provider(target_command: &Path, target_projects: &Path) -> ProviderConfig {
    claude_provider("claude", target_command, target_projects)
}

fn source_jsonl_path(source_projects: &Path, source_workspace: &Path) -> PathBuf {
    source_projects
        .join(claude_project_dir_name(source_workspace))
        .join(format!("{SESSION_ID}.jsonl"))
}

fn write_source_jsonl(path: &Path, source_workspace: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, format_source_jsonl(source_workspace)).unwrap();
}

fn format_source_jsonl(source_workspace: &Path) -> String {
    format!(
        "{{\"sessionId\":\"{SESSION_ID}\",\"type\":\"summary\",\"timestamp\":\"2026-04-17T08:00:00Z\"}}\n\
         {{\"sessionId\":\"{SESSION_ID}\",\"uuid\":\"turn-1\",\"type\":\"assistant\",\"timestamp\":\"2026-04-17T08:00:01Z\",\"cwd\":\"{}\"}}\n",
        source_workspace.display()
    )
}

fn start_source_invocation(db: &StateDb, model: &ModelConfig) -> i64 {
    db.start_invocation(&InvocationStart {
        invocation_uuid: uuid::Uuid::new_v4().to_string(),
        model_name: model.name.clone(),
        provider_name: "claude3".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
    .unwrap()
}

fn capture_source_session(db: &StateDb, invocation_id: i64) {
    db.update_session_capture(invocation_id, Some(SESSION_ID), "fixture")
        .unwrap();
}

fn mint_source_chain(db: &StateDb, invocation_id: i64) {
    db.mint_chain_for_invocation_session(invocation_id).unwrap();
}

fn model_store_for_fixture(model: &ModelConfig) -> ModelStore {
    HashMap::from([(model.name.clone(), model.clone())])
}

fn resolve_seeded_resume(db: &StateDb, models: &ModelStore, model: &ModelConfig) -> ResolvedResume {
    db.resolve_resume(models, SESSION_ID, Some(&model.name))
        .unwrap()
}

fn fake_claude_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fake-claude.sh")
}

fn fake_claude_script(target_projects: &Path) -> String {
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
        target_projects.display()
    )
}

fn mark_fake_provider_executable(script: &Path) {
    let mut perms = fs::metadata(script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(script, perms).unwrap();
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
        environment: Default::default(),
        unset_environment: Default::default(),
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
