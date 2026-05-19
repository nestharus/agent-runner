//! Declared roles: formatter, accessor, orchestration, mapper.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig,
};
use oulipoly_runtime::balancer::TransitionReason;
use oulipoly_runtime::migration::{MigratedSegment, MigrationError, migrate_chain_segment};
use oulipoly_state::{InvocationStart, ModelStore, ResolvedResume, StateDb};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub mod rc1_non_alnum_encoding;
pub mod rc2_windows_backslash_encoding;

#[cfg(test)]
pub mod age158_characterization;

#[cfg(unix)]
pub mod rc3_symlink_canonicalization;

pub const SESSION_ID: &str = "0de9435c-3727-49fd-998c-cd0ea2c177f7";

pub struct ClaudePathHashFixture {
    pub dir: tempfile::TempDir,
    pub source_projects: PathBuf,
    pub target_projects: PathBuf,
    pub source_workspace: PathBuf,
}

impl ClaudePathHashFixture {
    pub fn new() -> Self {
        let paths = claude_path_hash_fixture_paths();
        create_source_workspace(&paths.source_workspace);
        assemble_claude_path_hash_fixture(paths)
    }

    pub fn model(&self) -> ModelConfig {
        assemble_claude_path_hash_model(&self.source_projects, &self.target_projects)
    }

    pub fn state(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    pub fn seed_source_jsonl(&self) -> PathBuf {
        let path = source_jsonl_path(&self.source_projects, &self.source_workspace);
        write_source_jsonl(&path);
        path
    }

    pub fn seed_chain(&self, db: &StateDb, model: &ModelConfig) -> ResolvedResume {
        let invocation_id = start_source_invocation(db, model);
        capture_source_session(db, invocation_id);
        mint_source_chain(db, invocation_id);
        let models = model_store_for_fixture(model);
        resolve_seeded_resume(db, &models, model)
    }

    pub fn migrate_to(&self, resume_workspace: &Path) -> Result<MigratedSegment, MigrationError> {
        self.seed_source_jsonl();
        let model = self.model();
        let db = self.state();
        let resolved = self.seed_chain(&db, &model);
        run_path_hash_migration(&db, &model, &resolved, resume_workspace)
    }

    pub fn expected_target_path(&self, resume_workspace: &Path) -> PathBuf {
        self.target_projects
            .join(expected_claude_code_project_dir(resume_workspace))
            .join(format!("{SESSION_ID}.jsonl"))
    }

    pub fn path_with_non_alnum(&self) -> PathBuf {
        self.dir
            .path()
            .join("work_trees")
            .join("tmp.UfwcMhrgHV")
            .join("漢字_model")
    }
}

struct ClaudePathHashFixturePaths {
    dir: tempfile::TempDir,
    source_projects: PathBuf,
    target_projects: PathBuf,
    source_workspace: PathBuf,
}

fn claude_path_hash_fixture_paths() -> ClaudePathHashFixturePaths {
    let dir = tempfile::tempdir().unwrap();
    let source_projects = dir.path().join("claude-source").join("projects");
    let target_projects = dir.path().join("claude-target").join("projects");
    let source_workspace = dir.path().join("source-workspace");

    ClaudePathHashFixturePaths {
        dir,
        source_projects,
        target_projects,
        source_workspace,
    }
}

fn create_source_workspace(source_workspace: &Path) {
    fs::create_dir_all(source_workspace).unwrap();
}

fn assemble_claude_path_hash_fixture(paths: ClaudePathHashFixturePaths) -> ClaudePathHashFixture {
    ClaudePathHashFixture {
        dir: paths.dir,
        source_projects: paths.source_projects,
        target_projects: paths.target_projects,
        source_workspace: paths.source_workspace,
    }
}

fn assemble_claude_path_hash_model(source_projects: &Path, target_projects: &Path) -> ModelConfig {
    ModelConfig {
        name: "claude-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            claude_provider("claude-source", source_projects),
            claude_provider("claude-target", target_projects),
        ],
        inputs: Vec::new(),
    }
}

fn source_jsonl_path(source_projects: &Path, source_workspace: &Path) -> PathBuf {
    source_projects
        .join(expected_claude_code_project_dir(source_workspace))
        .join(format!("{SESSION_ID}.jsonl"))
}

fn write_source_jsonl(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, format_source_jsonl_turn()).unwrap();
}

fn format_source_jsonl_turn() -> String {
    format!(
        "{{\"sessionId\":\"{SESSION_ID}\",\"uuid\":\"turn-1\",\"type\":\"assistant\",\"timestamp\":\"2026-05-04T08:00:00Z\"}}\n"
    )
}

fn start_source_invocation(db: &StateDb, model: &ModelConfig) -> i64 {
    db.start_invocation(&source_invocation_start(model))
        .unwrap()
}

fn source_invocation_start(model: &ModelConfig) -> InvocationStart {
    InvocationStart {
        invocation_uuid: uuid::Uuid::new_v4().to_string(),
        model_name: model.name.clone(),
        provider_name: "claude-source".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
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

fn run_path_hash_migration(
    db: &StateDb,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    resume_workspace: &Path,
) -> Result<MigratedSegment, MigrationError> {
    let mut stderr = migration_stderr_buffer();
    migrate_chain_segment(
        db,
        &SessionsConfig::default(),
        model,
        resolved,
        resume_workspace,
        1,
        TransitionReason::Manual,
        &mut stderr,
    )
}

fn migration_stderr_buffer() -> Vec<u8> {
    Vec::new()
}

pub fn windows_shape_path() -> PathBuf {
    PathBuf::from(r"C:\Users\foo.bar\work_tree\漢字")
}

pub fn expected_claude_code_project_dir(path: &Path) -> String {
    path.to_string_lossy()
        .replace(['/', '\\'], "-")
        .chars()
        .map(|c| {
            if (c.is_ascii() && c.is_alphanumeric()) || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(unix)]
pub struct SymlinkWorkspace {
    pub real: PathBuf,
    pub link: PathBuf,
}

#[cfg(unix)]
pub fn symlinked_workspace(base: &Path) -> SymlinkWorkspace {
    let paths = symlink_workspace_paths(base);
    create_symlinked_workspace_dirs(&paths);
    assemble_symlink_workspace(paths)
}

#[cfg(unix)]
fn symlink_workspace_paths(base: &Path) -> SymlinkWorkspace {
    let real = base.join("real-workspace");
    let link = base.join("linked-workspace");
    SymlinkWorkspace { real, link }
}

#[cfg(unix)]
fn create_symlinked_workspace_dirs(paths: &SymlinkWorkspace) {
    fs::create_dir_all(&paths.real).unwrap();
    std::os::unix::fs::symlink(&paths.real, &paths.link).unwrap();
}

#[cfg(unix)]
fn assemble_symlink_workspace(paths: SymlinkWorkspace) -> SymlinkWorkspace {
    paths
}

fn claude_provider(name: &str, projects_dir: &Path) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        command: name.to_string(),
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
