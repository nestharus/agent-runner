//! ## Declared roles
//!
//! `validator`, `mapper`, `accessor`

mod provider_authority_fixture;

use agent_runner_lib::{AppState, derive_pools, save_model_inner, update_pool_inner};
use oulipoly_config::{self as config, ModelConfig, PromptMode, ProviderConfig};
use std::collections::HashMap;
use std::path::Path;

fn test_state(models_dir: std::path::PathBuf, models: HashMap<String, ModelConfig>) -> AppState {
    AppState::test_default(models_dir, models)
}

fn make_model(name: &str, commands: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: commands
            .iter()
            .map(|c| ProviderConfig::new(c.to_string(), vec![]))
            .collect(),
        inputs: vec![],
        provider: None,
    }
}

fn model_with_provider_args(name: &str, provider_name: &str, args: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::model_provider(
            provider_name,
            args.iter().map(|arg| (*arg).to_string()).collect(),
        )],
        inputs: vec![],
        provider: None,
    }
}

fn write_codex_providers(root: &Path) {
    std::fs::write(
        root.join("providers.toml"),
        provider_authority_fixture::with_explicit_provider_authority(
            r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
interactive_args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
        ),
    )
    .unwrap();
}

#[test]
fn save_model_inner_rejects_duplicate_codex_args_without_disk_or_memory_update() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    write_codex_providers(dir.path());
    let state = test_state(models_dir.clone(), HashMap::new());
    let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);

    let err = save_model_inner(&state, model).unwrap_err();

    assert!(err.contains("duplicates root [codex].args"), "{err}");
    assert!(!models_dir.join("gpt-high.toml").exists());
    assert!(!state.models.lock().unwrap().contains_key("gpt-high"));
}

#[test]
fn save_model_inner_accepts_clean_model_and_provider_aware_reload() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    write_codex_providers(dir.path());
    let providers = config::ProvidersConfig::load(&dir.path().join("providers.toml")).unwrap();
    let state = test_state(models_dir.clone(), HashMap::new());
    let model = model_with_provider_args(
        "gpt-high",
        "codex",
        &["-m", "gpt-5.5", "-c", "model_reasoning_effort=high"],
    );

    save_model_inner(&state, model).unwrap();

    assert!(models_dir.join("gpt-high.toml").exists());
    let loaded = config::load_models(&models_dir, Some(&providers)).unwrap();
    assert!(loaded.contains_key("gpt-high"));
}

#[test]
fn save_model_inner_accepts_duplicate_shape_without_sibling_providers() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    let state = test_state(models_dir.clone(), HashMap::new());
    let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);

    save_model_inner(&state, model).unwrap();

    assert!(models_dir.join("gpt-high.toml").exists());
    assert!(
        config::load_models(&models_dir, None)
            .unwrap()
            .contains_key("gpt-high")
    );
}

#[test]
fn save_model_inner_preserves_existing_basic_validation_errors() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path().join("models"), HashMap::new());

    let empty_name = model_with_provider_args("", "codex", &["-m", "gpt-5.5"]);
    assert_eq!(
        save_model_inner(&state, empty_name).unwrap_err(),
        "Model name cannot be empty"
    );

    let no_providers = ModelConfig {
        name: "gpt-high".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![],
        inputs: vec![],
        provider: None,
    };
    assert_eq!(
        save_model_inner(&state, no_providers).unwrap_err(),
        "Model must have at least one provider"
    );

    let empty_provider_name = model_with_provider_args("gpt-high", "", &["-m", "gpt-5.5"]);
    assert_eq!(
        save_model_inner(&state, empty_provider_name).unwrap_err(),
        "Provider 1 has empty name"
    );
}

#[test]
fn update_pool_inner_rejects_duplicate_preserving_rewrite_without_file_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    write_codex_providers(dir.path());
    let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);
    let model_path = models_dir.join("gpt-high.toml");
    std::fs::write(&model_path, "sentinel").unwrap();
    let state = test_state(models_dir, HashMap::from([(model.name.clone(), model)]));

    let err = update_pool_inner(
        &state,
        vec!["codex".to_string()],
        vec!["codex".to_string(), "claude".to_string()],
    )
    .unwrap_err();

    assert!(err.contains("duplicates root [codex].args"), "{err}");
    assert_eq!(std::fs::read_to_string(&model_path).unwrap(), "sentinel");
}

#[test]
fn update_pool_inner_accepts_clean_rewrite_and_added_provider_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    write_codex_providers(dir.path());
    let providers = config::ProvidersConfig::load(&dir.path().join("providers.toml")).unwrap();
    let model = model_with_provider_args("claude-high", "claude", &["--model", "sonnet"]);
    std::fs::write(models_dir.join("claude-high.toml"), model.to_toml()).unwrap();
    let state = test_state(
        models_dir.clone(),
        HashMap::from([(model.name.clone(), model)]),
    );

    update_pool_inner(
        &state,
        vec!["claude".to_string()],
        vec!["claude".to_string(), "codex".to_string()],
    )
    .unwrap();

    let loaded = config::load_models(&models_dir, Some(&providers)).unwrap();
    let codex = loaded["claude-high"]
        .providers
        .iter()
        .find(|provider| provider.name == "codex")
        .expect("codex provider was added");
    assert!(codex.args.is_empty());
}

#[test]
fn update_pool_inner_accepts_duplicate_preserving_rewrite_without_sibling_providers() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    let model = model_with_provider_args("gpt-high", "codex", &["exec", "-m", "gpt-5.5"]);
    std::fs::write(models_dir.join("gpt-high.toml"), model.to_toml()).unwrap();
    let state = test_state(
        models_dir.clone(),
        HashMap::from([(model.name.clone(), model)]),
    );

    update_pool_inner(
        &state,
        vec!["codex".to_string()],
        vec!["codex".to_string(), "claude".to_string()],
    )
    .unwrap();

    assert!(
        config::load_models(&models_dir, None)
            .unwrap()
            .contains_key("gpt-high")
    );
}

#[test]
fn update_pool_inner_preserves_existing_command_errors() {
    let dir = tempfile::tempdir().unwrap();
    let model = model_with_provider_args("claude-high", "claude", &["--model", "sonnet"]);
    let state = test_state(
        dir.path().join("models"),
        HashMap::from([(model.name.clone(), model)]),
    );

    assert_eq!(
        update_pool_inner(&state, vec!["claude".to_string()], vec![]).unwrap_err(),
        "Pool must have at least one command"
    );
    assert_eq!(
        update_pool_inner(
            &state,
            vec!["codex".to_string()],
            vec!["codex".to_string(), "claude".to_string()],
        )
        .unwrap_err(),
        "No models found with the specified command set"
    );
}

#[test]
fn derive_pools_groups_by_command_set() {
    let mut models = HashMap::new();
    models.insert("a".into(), make_model("a", &["claude", "codex"]));
    models.insert("b".into(), make_model("b", &["claude", "codex"]));
    models.insert("c".into(), make_model("c", &["gemini"]));

    let pools = derive_pools(&models);
    assert_eq!(pools.len(), 2);

    let pool_claude = pools
        .iter()
        .find(|p| p.commands.contains(&"claude".to_string()))
        .unwrap();
    assert_eq!(pool_claude.model_count, 2);
    assert!(pool_claude.model_names.contains(&"a".to_string()));
    assert!(pool_claude.model_names.contains(&"b".to_string()));

    let pool_gemini = pools
        .iter()
        .find(|p| p.commands.contains(&"gemini".to_string()))
        .unwrap();
    assert_eq!(pool_gemini.model_count, 1);
    assert_eq!(pool_gemini.model_names, vec!["c".to_string()]);
}

#[test]
fn derive_pools_deduplicates_commands() {
    let mut models = HashMap::new();
    models.insert(
        "x".into(),
        ModelConfig {
            name: "x".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![
                ProviderConfig::new("claude", vec![]),
                ProviderConfig::new("claude", vec!["-p".to_string()]),
            ],
            inputs: vec![],
            provider: None,
        },
    );
    models.insert("y".into(), make_model("y", &["claude"]));

    let pools = derive_pools(&models);
    assert_eq!(pools.len(), 1);
    assert_eq!(pools[0].model_count, 2);
}

#[test]
fn derive_pools_groups_by_provider_name() {
    let mut models = HashMap::new();
    models.insert(
        "a".into(),
        ModelConfig {
            name: "a".to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider("claude", vec![])],
            inputs: vec![],
            provider: None,
        },
    );
    models.insert("b".into(), make_model("b", &["claude"]));

    let pools = derive_pools(&models);
    assert_eq!(pools.len(), 1);
    assert_eq!(pools[0].commands, vec!["claude".to_string()]);
    assert_eq!(pools[0].model_count, 2);
}
