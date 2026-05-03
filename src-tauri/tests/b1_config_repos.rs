#![cfg(unix)]

mod fixtures;

use agent_runner_lib::config::{
    AgentConfigRepository, ModelConfigRepository, ProviderConfigSource, SessionsConfigSource,
};
use fixtures::b1_config_repos::ConfigRepoFixture;

/// Risk: T7/T8 - lifting model filesystem access into a repository may move
/// filename-derived names into parser behavior or start reading non-TOML files.
/// Level: component.
/// Source: proposal section 8 T7/T8; contract section 5 `ModelConfigRepository`.
/// Observable: repository loading keys models by `.toml` file stem and ignores
/// non-TOML siblings.
#[test]
fn model_config_repository_load_models_uses_filename_derived_names() {
    let fixture = ConfigRepoFixture::new();
    fixture.write_model_file(
        "filename-derived.toml",
        r#"[[providers]]
name = "claude"
"#,
    );
    fixture.write_model_file("ignored.txt", "not toml");

    let repo = fixture.model_repo();
    let repo: &dyn ModelConfigRepository = &repo;
    let models = repo.load_models().unwrap();

    assert!(models.contains_key("filename-derived"));
    assert!(!models.contains_key("ignored"));
    assert_eq!(models["filename-derived"].name, "filename-derived");
}

/// Risk: T7/T8 - provider source extraction may change the current missing-file
/// default behavior into a startup error.
/// Level: component.
/// Source: proposal section 8 T7/T8; contract section 5 `ProviderConfigSource`.
/// Observable: absent `providers.toml` loads as the default empty config.
#[test]
fn provider_config_source_missing_file_returns_default_empty_config() {
    let fixture = ConfigRepoFixture::new();
    let source = fixture.provider_source();
    let source: &dyn ProviderConfigSource = &source;

    let providers = source.load_providers().unwrap();

    assert!(providers.runtime_provider("claude").is_none());
}

/// Risk: T7/T8 - sessions source extraction may change absent sessions config
/// into an error or add new validation beyond deserialization.
/// Level: component.
/// Source: proposal section 8 T7/T8; contract section 5 `SessionsConfigSource`.
/// Observable: absent `sessions.toml` loads as the default empty config.
#[test]
fn sessions_config_source_missing_file_returns_default_empty_config() {
    let fixture = ConfigRepoFixture::new();
    let source = fixture.sessions_source();
    let source: &dyn SessionsConfigSource = &source;

    let sessions = source.load_sessions().unwrap();

    assert!(sessions.get("claude").is_none());
}

/// Risk: T7/T8 - agent repository extraction may start parsing non-agent files
/// in the default agents directory.
/// Level: component.
/// Source: proposal section 8 T7/T8; contract section 5 `AgentConfigRepository`.
/// Observable: non-`.md` files are ignored by directory loading.
#[test]
fn agent_config_repository_load_agents_ignores_non_markdown_files() {
    let fixture = ConfigRepoFixture::new();
    fixture.write_agent_file("not-an-agent.txt", "---\nmodel: claude\n---\nbody\n");

    let repo = fixture.agent_repo();
    let repo: &dyn AgentConfigRepository = &repo;
    let agents = repo.load_agents().unwrap();

    assert!(agents.is_empty());
}
