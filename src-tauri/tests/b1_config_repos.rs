#![cfg(unix)]

mod fixtures;

use agent_runner_lib::config::{
    AgentConfigRepository, FilesystemAgentConfigRepository, FilesystemModelConfigRepository,
    FilesystemProviderConfigSource, FilesystemSessionsConfigSource, ModelConfigRepository,
    ProviderConfigSource, SessionsConfigSource,
};
use fixtures::b1_config_repos::*;

/// Risk: T7 (model config repository preserves filesystem load semantics)
/// Source: proposal §8 T7; contract §5 ModelConfigRepository; assumption A4
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_load_models_returns_empty_for_missing_directory() {
    let fixture = ConfigRepoFixture::new();
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    let models = repo.load_models().unwrap();

    assert!(models.is_empty());
}

/// Risk: T7/T16 (filename-derived model names remain repository behavior)
/// Source: proposal §8 T7/T16; contract §5 ModelConfigRepository; assumption A4
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_load_models_reads_only_toml_and_keys_by_file_stem() {
    let fixture = ConfigRepoFixture::new();
    fixture.write_model_toml("from-file-stem", &single_provider_model_toml("claude"));
    std::fs::write(
        fixture.models_dir().join("ignored.txt"),
        single_provider_model_toml("codex"),
    )
    .unwrap();
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    let models = repo.load_models().unwrap();

    assert_eq!(models.len(), 1);
    let model = models.get("from-file-stem").unwrap();
    assert_eq!(model.name, "from-file-stem");
    assert_eq!(model.providers[0].name, "claude");
}

/// Risk: T7 (invalid model config filenames surface as repository errors)
/// Source: proposal §8 T7; contract §5 ModelConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_load_models_errors_on_invalid_filename() {
    let fixture = ConfigRepoFixture::new();
    fixture.write_non_utf8_model_filename();
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    let err = repo.load_models().unwrap_err();

    assert!(err.contains("filename") || err.contains("UTF"), "{err}");
}

/// Risk: T7/T8 (model save persists TOML without mutating caller registries)
/// Source: proposal §8 T7/T8; contract §5 ModelConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_save_model_creates_directory_and_writes_toml() {
    let fixture = ConfigRepoFixture::new();
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    repo.save_model(&model("saved", &["claude", "codex"]))
        .unwrap();

    let written = std::fs::read_to_string(fixture.models_dir().join("saved.toml")).unwrap();
    assert!(written.contains("[[providers]]"), "{written}");
    assert!(written.contains("claude"), "{written}");
    assert!(written.contains("codex"), "{written}");
}

/// Risk: T7/T8 (model save cannot escape configured models directory)
/// Source: CodeRabbit Phase 7 pass 4; contract §5 ModelConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_save_model_rejects_path_traversal_name() {
    let fixture = ConfigRepoFixture::new();
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    let err = repo
        .save_model(&model("../escaped", &["claude"]))
        .unwrap_err();

    assert!(err.contains("Invalid model name"), "{err}");
    assert!(!fixture.root().join("escaped.toml").exists());
}

/// Risk: T7/T8 (model save rejects platform-unsafe filenames)
/// Source: CodeRabbit Phase 7 pass 6; contract §5 ModelConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_save_model_rejects_platform_unsafe_names() {
    let fixture = ConfigRepoFixture::new();
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    for name in [
        "",
        " ",
        "C:escaped",
        "CON",
        "nul.txt",
        "trailing.",
        "trailing ",
    ] {
        let err = repo.save_model(&model(name, &["claude"])).unwrap_err();
        assert!(err.contains("Invalid model name"), "{name}: {err}");
    }
}

/// Risk: T7/T8 (model save reports filesystem write failures)
/// Source: proposal §8 T7/T8; contract §5 ModelConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_save_model_errors_when_directory_cannot_be_created() {
    let fixture = ConfigRepoFixture::new();
    let blocked_models_dir = fixture.block_models_dir_creation();
    let repo = FilesystemModelConfigRepository::new(blocked_models_dir);
    let repo: &dyn ModelConfigRepository = &repo;

    let err = repo.save_model(&model("blocked", &["claude"])).unwrap_err();

    assert!(
        err.contains("directory") || err.contains("Not a directory"),
        "{err}"
    );
}

/// Risk: T7/T8 (model delete keeps missing-delete command semantics)
/// Source: proposal §8 T7/T8; contract §5 ModelConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_delete_model_removes_file_and_succeeds_when_absent() {
    let fixture = ConfigRepoFixture::new();
    fixture.write_model_toml("removable", &single_provider_model_toml("claude"));
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    repo.delete_model("removable").unwrap();
    repo.delete_model("removable").unwrap();

    assert!(!fixture.models_dir().join("removable.toml").exists());
}

/// Risk: T7/T8 (model delete cannot escape configured models directory)
/// Source: CodeRabbit Phase 7 pass 5; contract §5 ModelConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn model_config_repository_delete_model_rejects_path_traversal_name() {
    let fixture = ConfigRepoFixture::new();
    let escaped = fixture.root().join("escaped.toml");
    std::fs::write(&escaped, "must remain").unwrap();
    let repo = FilesystemModelConfigRepository::new(fixture.models_dir().to_path_buf());
    let repo: &dyn ModelConfigRepository = &repo;

    let err = repo.delete_model("../escaped").unwrap_err();

    assert!(err.contains("Invalid model name"), "{err}");
    assert_eq!(std::fs::read_to_string(escaped).unwrap(), "must remain");
}

/// Risk: T7/T14/T15/T16 (provider source missing-file default is preserved)
/// Source: proposal §8 T7/T14/T15/T16; contract §5 ProviderConfigSource
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn provider_config_source_missing_file_returns_default_config() {
    let fixture = ConfigRepoFixture::new();
    let source = FilesystemProviderConfigSource::new(fixture.providers_path().to_path_buf());
    let source: &dyn ProviderConfigSource = &source;

    let providers = source.load_providers().unwrap();

    assert!(providers.runtime_provider("claude").is_none());
}

/// Risk: T7/T14/T15/T16 (provider source reads existing TOML without absorbing pure merge methods)
/// Source: proposal §8 T7/T14/T15/T16; contract §5 ProviderConfigSource
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn provider_config_source_loads_existing_file_and_keeps_effective_provider_pure() {
    let fixture = ConfigRepoFixture::new();
    let storage_root = fixture.root().join("projects");
    fixture.write_providers_toml(&provider_config_toml(&storage_root));
    let source = FilesystemProviderConfigSource::new(fixture.providers_path().to_path_buf());
    let source: &dyn ProviderConfigSource = &source;

    let providers = source.load_providers().unwrap();
    let runtime = providers.runtime_provider("claude").unwrap();

    assert_eq!(runtime.command, "claude");
    assert_eq!(runtime.args, vec!["--fast".to_string()]);
}

/// Risk: T7/T14/T15/T16 (provider source surfaces malformed TOML)
/// Source: proposal §8 T7/T14/T15/T16; contract §5 ProviderConfigSource
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn provider_config_source_malformed_file_returns_error() {
    let fixture = ConfigRepoFixture::new();
    fixture.write_providers_toml("[claude\ncommand = ");
    let source = FilesystemProviderConfigSource::new(fixture.providers_path().to_path_buf());
    let source: &dyn ProviderConfigSource = &source;

    let err = source.load_providers().unwrap_err();

    assert!(err.contains("TOML") || err.contains("parse"), "{err}");
}

/// Risk: T7/T9/T11/T13 (sessions source missing-file default is preserved)
/// Source: proposal §8 T7/T9/T11/T13; contract §5 SessionsConfigSource
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn sessions_config_source_missing_file_returns_default_config() {
    let fixture = ConfigRepoFixture::new();
    let source = FilesystemSessionsConfigSource::new(fixture.sessions_path().to_path_buf());
    let source: &dyn SessionsConfigSource = &source;

    let sessions = source.load_sessions().unwrap();

    assert!(sessions.get("claude").is_none());
}

/// Risk: T7/T9/T11/T13 (sessions source preserves state_dir tilde expansion)
/// Source: proposal §8 T7/T9/T11/T13; contract §5 SessionsConfigSource; assumption A4
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn sessions_config_source_loads_existing_file_and_expands_home_state_dir() {
    let fixture = ConfigRepoFixture::new();
    let home = fixture.root().join("home");
    fixture.write_sessions_toml(sessions_config_toml());
    let source = FilesystemSessionsConfigSource::new(fixture.sessions_path().to_path_buf());
    let source: &dyn SessionsConfigSource = &source;

    isolated_home(&home, || {
        let sessions = source.load_sessions().unwrap();
        let entry = sessions.get("claude").unwrap();

        assert_eq!(entry.turn_script, "cat transcript.jsonl");
        assert_eq!(
            entry.state_dir.as_ref().unwrap(),
            &home.join("oulipoly-state")
        );
    });
}

/// Risk: T7/T9/T11/T13 (sessions source surfaces malformed TOML)
/// Source: proposal §8 T7/T9/T11/T13; contract §5 SessionsConfigSource
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn sessions_config_source_malformed_file_returns_error() {
    let fixture = ConfigRepoFixture::new();
    fixture.write_sessions_toml("[claude\nturn_script = ");
    let source = FilesystemSessionsConfigSource::new(fixture.sessions_path().to_path_buf());
    let source: &dyn SessionsConfigSource = &source;

    let err = source.load_sessions().unwrap_err();

    assert!(err.contains("TOML") || err.contains("parse"), "{err}");
}

/// Risk: T7/T13 (agent repository explicit file loading derives filename stem)
/// Source: proposal §8 T7/T13; contract §5 AgentConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn agent_config_repository_load_agent_file_derives_name_from_file_stem() {
    let fixture = ConfigRepoFixture::new();
    let path = fixture.write_agent_file("writer", &agent_markdown("fixture-model"));
    let repo = FilesystemAgentConfigRepository::new(fixture.agents_dir().to_path_buf());
    let repo: &dyn AgentConfigRepository = &repo;

    let agent = repo.load_agent_file(&path).unwrap();

    assert_eq!(agent.name, "writer");
    assert_eq!(agent.model, "fixture-model");
}

/// Risk: T7/T13 (agent repository directory loading ignores non-md files)
/// Source: proposal §8 T7/T13; contract §5 AgentConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn agent_config_repository_load_agents_empty_for_missing_dir_and_ignores_non_md_files() {
    let fixture = ConfigRepoFixture::new();
    let repo = FilesystemAgentConfigRepository::new(fixture.agents_dir().to_path_buf());
    let repo: &dyn AgentConfigRepository = &repo;
    assert!(repo.load_agents().unwrap().is_empty());

    fixture.write_non_md_agent_file("ignored");
    fixture.write_agent_file("kept", &agent_markdown("fixture-model"));

    let agents = repo.load_agents().unwrap();
    assert_eq!(agents.len(), 1);
    assert!(agents.contains_key("kept"));
}

/// Risk: T7/T13 (agent repository surfaces file parse errors)
/// Source: proposal §8 T7/T13; contract §5 AgentConfigRepository
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b1_config_repos.rs
#[test]
fn agent_config_repository_missing_frontmatter_returns_error() {
    let fixture = ConfigRepoFixture::new();
    let path = fixture.write_agent_file("broken", "# Missing frontmatter\n");
    let repo = FilesystemAgentConfigRepository::new(fixture.agents_dir().to_path_buf());
    let repo: &dyn AgentConfigRepository = &repo;

    let err = repo.load_agent_file(&path).unwrap_err();

    assert!(err.contains("frontmatter") || err.contains("---"), "{err}");
}
