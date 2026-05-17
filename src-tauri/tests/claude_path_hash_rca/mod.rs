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
        let dir = tempfile::tempdir().unwrap();
        let source_projects = dir.path().join("claude-source").join("projects");
        let target_projects = dir.path().join("claude-target").join("projects");
        let source_workspace = dir.path().join("source-workspace");
        fs::create_dir_all(&source_workspace).unwrap();
        Self {
            dir,
            source_projects,
            target_projects,
            source_workspace,
        }
    }

    pub fn model(&self) -> ModelConfig {
        ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                claude_provider("claude-source", &self.source_projects),
                claude_provider("claude-target", &self.target_projects),
            ],
            inputs: Vec::new(),
        }
    }

    pub fn state(&self) -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    pub fn seed_source_jsonl(&self) -> PathBuf {
        let source_dir = self
            .source_projects
            .join(expected_claude_code_project_dir(&self.source_workspace));
        fs::create_dir_all(&source_dir).unwrap();
        let path = source_dir.join(format!("{SESSION_ID}.jsonl"));
        fs::write(
            &path,
            format!(
                "{{\"sessionId\":\"{SESSION_ID}\",\"uuid\":\"turn-1\",\"type\":\"assistant\",\"timestamp\":\"2026-05-04T08:00:00Z\"}}\n"
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
                provider_name: "claude-source".to_string(),
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

    pub fn migrate_to(&self, resume_workspace: &Path) -> Result<MigratedSegment, MigrationError> {
        self.seed_source_jsonl();
        let model = self.model();
        let db = self.state();
        let resolved = self.seed_chain(&db, &model);
        let mut stderr = Vec::new();
        migrate_chain_segment(
            &db,
            &SessionsConfig::default(),
            &model,
            &resolved,
            resume_workspace,
            1,
            TransitionReason::Manual,
            &mut stderr,
        )
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
    let real = base.join("real-workspace");
    let link = base.join("linked-workspace");
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    SymlinkWorkspace { real, link }
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
