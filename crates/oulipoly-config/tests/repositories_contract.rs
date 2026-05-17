use oulipoly_config::app::AppConfig;
use oulipoly_config::repositories::{
    AgentConfigRepository, AppConfigRepository, FilesystemAgentConfigRepository,
    FilesystemAppConfigRepository, FilesystemModelConfigRepository,
    FilesystemProvidersConfigRepository, FilesystemSessionsConfigRepository, ModelConfigRepository,
    ProvidersConfigRepository, SessionsConfigRepository,
};
use oulipoly_config::{
    ClaudeRestrictions, CodexRestrictions, InvocationMode, ModelConfig, PromptMode, ProviderConfig,
    ProvidersConfig, SessionsConfig, ToolRestrictionKind, ToolRestrictions, load_agent_file,
    load_agents, load_models,
};
use std::fs;
use std::path::{Path, PathBuf};

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn model_config(name: &str, provider_name: &str) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::model_provider(provider_name, vec![])],
        inputs: vec![],
    }
}

#[test]
fn app_config_repository_delegates_app_config_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    write(
        &path,
        r#"
diagnostics_model = "diagnostic"
default_provider = "codex"
"#,
    );
    let repo = FilesystemAppConfigRepository;

    let direct = AppConfig::load(&path).unwrap();
    let via_trait =
        <FilesystemAppConfigRepository as AppConfigRepository>::load_app_config(&repo, &path)
            .unwrap();

    assert_eq!(via_trait.diagnostics_model, direct.diagnostics_model);
    assert_eq!(via_trait.default_provider, direct.default_provider);
}

#[test]
fn agent_config_repository_delegates_agent_loaders() {
    let dir = tempfile::tempdir().unwrap();
    let agents_dir = dir.path().join("agents");
    let agent_path = agents_dir.join("reviewer.md");
    write(
        &agent_path,
        "---\ndescription: 'Reviews code'\nmodel: claude~high\noutput_format: markdown\n---\nRead carefully.\n",
    );
    write(&agents_dir.join("ignored.txt"), "not an agent");
    let repo = FilesystemAgentConfigRepository;

    let direct_file = load_agent_file(&agent_path).unwrap();
    let trait_file = <FilesystemAgentConfigRepository as AgentConfigRepository>::load_agent_file(
        &repo,
        &agent_path,
    )
    .unwrap();
    assert_eq!(trait_file.name, direct_file.name);
    assert_eq!(trait_file.description, direct_file.description);
    assert_eq!(trait_file.model, direct_file.model);
    assert_eq!(trait_file.instructions, direct_file.instructions);

    let direct_agents = load_agents(&agents_dir).unwrap();
    let trait_agents =
        <FilesystemAgentConfigRepository as AgentConfigRepository>::load_agents(&repo, &agents_dir)
            .unwrap();
    assert_eq!(
        trait_agents.keys().collect::<Vec<_>>(),
        direct_agents.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        trait_agents["reviewer"].model,
        direct_agents["reviewer"].model
    );
}

#[test]
fn model_config_repository_delegates_load_save_list_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    fs::create_dir_all(&models_dir).unwrap();
    let repo = FilesystemModelConfigRepository;
    let model = model_config("claude-high", "claude");

    <FilesystemModelConfigRepository as ModelConfigRepository>::save_model(
        &repo,
        &models_dir,
        &model,
    )
    .unwrap();

    let direct_loaded = load_models(&models_dir, None).unwrap();
    let trait_loaded =
        <FilesystemModelConfigRepository as ModelConfigRepository>::load_models(&repo, &models_dir)
            .unwrap();
    assert_eq!(
        trait_loaded.keys().collect::<Vec<_>>(),
        direct_loaded.keys().collect::<Vec<_>>()
    );
    assert_eq!(trait_loaded["claude-high"].providers[0].name, "claude");

    let mut direct_files = fs::read_dir(&models_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<PathBuf>>();
    direct_files.sort();
    let mut trait_files =
        <FilesystemModelConfigRepository as ModelConfigRepository>::list_model_files(
            &repo,
            &models_dir,
        )
        .unwrap();
    trait_files.sort();
    assert_eq!(trait_files, direct_files);

    <FilesystemModelConfigRepository as ModelConfigRepository>::delete_model_file(
        &repo,
        &models_dir,
        "claude-high",
    )
    .unwrap();
    assert!(!models_dir.join("claude-high.toml").exists());
}

#[test]
fn repository_load_models_validates_provider_overlap_by_contract() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    fs::create_dir_all(&models_dir).unwrap();
    let repo = FilesystemModelConfigRepository;
    let model = ModelConfig {
        name: "gpt-high".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::model_provider(
            "codex",
            vec!["exec".to_string(), "-m".to_string(), "gpt-5.5".to_string()],
        )],
        inputs: vec![],
    };
    write(
        &dir.path().join("providers.toml"),
        r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
"#,
    );
    let providers = ProvidersConfig::load(&dir.path().join("providers.toml")).unwrap();

    <FilesystemModelConfigRepository as ModelConfigRepository>::save_model(
        &repo,
        &models_dir,
        &model,
    )
    .unwrap();

    let trait_loaded =
        <FilesystemModelConfigRepository as ModelConfigRepository>::load_models(&repo, &models_dir)
            .unwrap();
    assert!(trait_loaded.contains_key("gpt-high"));
    assert!(
        load_models(&models_dir, None)
            .unwrap()
            .contains_key("gpt-high")
    );

    let err = load_models(&models_dir, Some(&providers)).unwrap_err();
    assert!(err.contains("duplicates root [codex].args"), "{err}");
}

#[test]
fn providers_config_repository_delegates_load_and_provider_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    write(
        &path,
        r#"
[claude]
command = "env"
args = ["-u", "CLAUDECODE", "claude"]
interactive_args = ["-u", "CLAUDECODE", "claude"]
prompt_mode = "stdin"

[codex]
command = "codex"
args = ["run"]
prompt_mode = "arg"
"#,
    );
    let repo = FilesystemProvidersConfigRepository;

    let direct = ProvidersConfig::load(&path).unwrap();
    let via_trait =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::load_providers(
            &repo, &path,
        )
        .unwrap();
    assert_eq!(via_trait.entries.len(), direct.entries.len());

    let model_provider = ProviderConfig::model_provider("claude", vec!["--model".to_string()]);
    let (direct_effective, direct_mode) = direct.effective_provider(&model_provider).unwrap();
    let (trait_effective, trait_mode) =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::effective_provider(
            &repo,
            &via_trait,
            &model_provider,
        )
        .unwrap();
    assert_eq!(trait_effective.command, direct_effective.command);
    assert_eq!(trait_effective.args, direct_effective.args);
    assert_eq!(trait_mode, direct_mode);

    let (direct_runtime, direct_runtime_mode) = direct.runtime_provider("codex").unwrap();
    let (trait_runtime, trait_runtime_mode) =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::runtime_provider(
            &repo, &via_trait, "codex",
        )
        .unwrap();
    assert_eq!(trait_runtime.command, direct_runtime.command);
    assert_eq!(trait_runtime.args, direct_runtime.args);
    assert_eq!(trait_runtime_mode, direct_runtime_mode);
}

#[test]
fn repository_effective_provider_preserves_age28_policy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    write(
        &path,
        r#"
[claude]
command = "env -u CLAUDECODE claude"
args = ["-p", "--root"]
interactive_args = ["--interactive-root"]
prompt_mode = "stdin"
system_prompt_override = "repository effective override"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task"]
disable_slash_commands = true
"#,
    );
    let repo = FilesystemProvidersConfigRepository;
    let direct = ProvidersConfig::load(&path).unwrap();
    let via_trait =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::load_providers(
            &repo, &path,
        )
        .unwrap();
    let model_provider =
        ProviderConfig::model_provider("claude", vec!["--model".to_string(), "opus".to_string()]);

    let (direct_effective, direct_mode) = direct.effective_provider(&model_provider).unwrap();
    let (trait_effective, trait_mode) =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::effective_provider(
            &repo,
            &via_trait,
            &model_provider,
        )
        .unwrap();

    assert_eq!(trait_mode, direct_mode);
    assert_eq!(trait_effective.args, direct_effective.args);
    assert_eq!(
        trait_effective.system_prompt_override,
        direct_effective.system_prompt_override
    );
    assert_eq!(
        trait_effective.tool_restrictions,
        Some(ToolRestrictions {
            kind: ToolRestrictionKind::Claude,
            claude: ClaudeRestrictions {
                disallowed_tools: vec!["Task".to_string()],
                allowed_tools: Vec::new(),
                disable_slash_commands: true,
            },
            codex: CodexRestrictions::default(),
        })
    );
    assert_eq!(
        trait_effective.tool_restrictions,
        direct_effective.tool_restrictions
    );
}

#[test]
fn repository_runtime_provider_preserves_age28_policy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    write(
        &path,
        r#"
[codex]
command = "codex"
args = ["exec"]
interactive_args = ["exec"]
prompt_mode = "arg"
system_prompt_override = "repository runtime override"

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.codex]
disabled_features = []
"#,
    );
    let repo = FilesystemProvidersConfigRepository;
    let direct = ProvidersConfig::load(&path).unwrap();
    let via_trait =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::load_providers(
            &repo, &path,
        )
        .unwrap();

    let (direct_runtime, direct_mode) = direct.runtime_provider("codex").unwrap();
    let (trait_runtime, trait_mode) =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::runtime_provider(
            &repo, &via_trait, "codex",
        )
        .unwrap();

    assert_eq!(trait_mode, direct_mode);
    assert_eq!(
        trait_runtime.system_prompt_override.as_deref(),
        Some("repository runtime override")
    );
    assert_eq!(
        trait_runtime.system_prompt_override,
        direct_runtime.system_prompt_override
    );
    assert_eq!(
        trait_runtime.tool_restrictions,
        direct_runtime.tool_restrictions
    );
}

#[test]
fn repository_effective_provider_preserves_invocation_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    write(
        &path,
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    );
    let repo = FilesystemProvidersConfigRepository;
    let providers =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::load_providers(
            &repo, &path,
        )
        .unwrap();
    let model_provider = ProviderConfig::model_provider("claude", vec!["--model".to_string()]);

    let (effective, _) =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::effective_provider(
            &repo,
            &providers,
            &model_provider,
        )
        .unwrap();

    assert_eq!(effective.invocation_mode, InvocationMode::Proxy);
}

#[test]
fn repository_runtime_provider_preserves_invocation_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.toml");
    write(
        &path,
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    );
    let repo = FilesystemProvidersConfigRepository;
    let providers =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::load_providers(
            &repo, &path,
        )
        .unwrap();

    let (runtime, _) =
        <FilesystemProvidersConfigRepository as ProvidersConfigRepository>::runtime_provider(
            &repo, &providers, "claude",
        )
        .unwrap();

    assert_eq!(runtime.invocation_mode, InvocationMode::Proxy);
}

#[test]
fn default_nestharus_policy_fixture_covers_bug1_and_bug2() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let fixture = workspace
        .join("tests")
        .join("fixtures")
        .join("age28-default-policy.providers.toml");
    let content = fs::read_to_string(&fixture).unwrap();

    let config = ProvidersConfig::load(&fixture).unwrap();
    for alias in [
        "claude", "claude2", "claude3", "claude4", "claude5", "claude6", "codex", "codex2",
        "codex3",
    ] {
        assert!(content.contains(&format!("[{alias}]")), "missing {alias}");
        let entry = config
            .get(alias)
            .unwrap_or_else(|| panic!("missing parsed provider {alias}"));
        assert!(
            entry
                .system_prompt_override
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
            "missing system_prompt_override for {alias}"
        );
        let restrictions = entry
            .tool_restrictions
            .as_ref()
            .unwrap_or_else(|| panic!("missing tool_restrictions for {alias}"));
        let expected_kind = if alias.starts_with("claude") {
            ToolRestrictionKind::Claude
        } else {
            ToolRestrictionKind::Codex
        };
        assert_eq!(restrictions.kind, expected_kind, "wrong kind for {alias}");
    }

    let lower = content.to_ascii_lowercase();
    assert!(lower.contains("task tool"), "{content}");
    assert!(lower.contains("bare agents"), "{content}");
    assert!(lower.contains("agents -m"), "{content}");
    assert!(lower.contains("-f <prompt-file>"), "{content}");
}

#[test]
fn sessions_config_repository_delegates_load_and_source_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.toml");
    write(
        &path,
        r#"
[claude]
turn_script = "claude-code-turns ~/.claude/projects"
transcript_locator = "claude-code-locate ~/.claude/projects"
"#,
    );
    let repo = FilesystemSessionsConfigRepository;

    let direct = SessionsConfig::load(&path).unwrap();
    let via_trait =
        <FilesystemSessionsConfigRepository as SessionsConfigRepository>::load_sessions(
            &repo, &path,
        )
        .unwrap();
    assert_eq!(via_trait.entries.len(), direct.entries.len());

    let direct_source = direct.get("claude").unwrap();
    let trait_source =
        <FilesystemSessionsConfigRepository as SessionsConfigRepository>::get_source(
            &repo, &via_trait, "claude",
        )
        .unwrap();
    assert_eq!(trait_source.turn_script, direct_source.turn_script);
    assert_eq!(
        trait_source.transcript_locator,
        direct_source.transcript_locator
    );
}
